use redis::AsyncCommands;
use std::time::Duration;
use tokio::net::TcpStream;

/// Watchdog that probes each backend over TCP and publishes the result on Redis.
///
/// Redis key convention (the watchdog is the ONLY writer for these keys):
///   * `health:<addr>` -> "1" if the backend is reachable, "0" otherwise.
///
/// The watchdog **never** writes to `weight:<addr>` — that key belongs to the
/// human operator. The load balancer combines both at routing time:
///
///     effective_weight = weight × health
///
/// This separation fixes two real bugs:
///   - Setting `weight:<addr>` 0 from redis-cli used to be undone by the
///     watchdog as soon as the next health probe succeeded. Now it sticks.
///   - A backend that was healthy but operator-disabled (weight=0) still
///     reports `health=1`, so flipping the weight back to N immediately puts
///     it back in rotation without waiting for the next probe.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting Health Checker Watchdog...");

    let backends_env = std::env::var("BACKENDS").unwrap_or_else(|_| {
        "127.0.0.1:8081,127.0.0.1:8082,127.0.0.1:8083,127.0.0.1:8084".to_string()
    });
    let backends: Vec<&str> = backends_env.split(',').map(|s| s.trim()).collect();

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
    let client = redis::Client::open(redis_url)?;

    loop {
        match client.get_multiplexed_tokio_connection().await {
            Ok(mut con) => loop {
                for &addr in backends.iter() {
                    let health_key = format!("health:{}", addr);
                    let is_healthy = check_health(addr).await;
                    let new_val: u8 = if is_healthy { 1 } else { 0 };

                    // Read previous value so we only log on transitions.
                    let prev: Result<u8, _> = con.get(&health_key).await;
                    let _: Result<(), _> = con.set(&health_key, new_val).await;

                    match (prev, is_healthy) {
                        (Ok(0), true) | (Err(_), true) => {
                            println!("Backend {} is UP (health=1).", addr);
                        }
                        (Ok(1), false) => {
                            println!("Backend {} is DOWN (health=0).", addr);
                        }
                        _ => {} // unchanged, stay quiet
                    }
                }

                tokio::time::sleep(Duration::from_secs(2)).await;
            },
            Err(e) => {
                println!("Failed to connect to Redis: {}. Retrying in 5s...", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn check_health(addr: &str) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}
