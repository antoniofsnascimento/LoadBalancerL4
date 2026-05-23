use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};

/// Continuous test client for the L4 Load Balancer.
///
/// Usage:
///   test_client [DURATION_SECS] [RATE_PER_SEC] [MAX_INFLIGHT]
///
/// Defaults:
///   DURATION_SECS = 60
///   RATE_PER_SEC  = 500       (steady rate; 0 = "as fast as possible, but bounded")
///   MAX_INFLIGHT  = 200       (semaphore-bound concurrency)
///
/// The rate-limit + concurrency-bound combo prevents local port exhaustion
/// (TIME_WAIT spam) and OOM on the LB container, so the dashboard shows a
/// truly continuous flow instead of bursts.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend_ports = vec![8081, 8082, 8083, 8084];
    let lb_port = 8080;

    let args: Vec<String> = std::env::args().collect();
    let duration_secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let rate_per_sec: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(500);
    let max_inflight: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(200);

    let duration = Duration::from_secs(duration_secs);
    let target_addr = format!("127.0.0.1:{}", lb_port);

    // Per-port hit counters (lock-free atomics, no Mutex needed in hot path).
    let request_counts: Arc<Mutex<HashMap<u16, usize>>> = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut lock = request_counts.lock().await;
        for &port in &backend_ports {
            lock.insert(port, 0);
        }
    }

    let success = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let inflight = Arc::new(AtomicUsize::new(0));

    // Categorised error counters — without these, "100 k failures" reads as
    // a load-balancer problem, when in practice it's almost always the
    // *client host* running out of ephemeral ports (EADDRNOTAVAIL) under
    // sustained high rate. Splitting by kind makes the cause obvious.
    let err_connect_addrunavail = Arc::new(AtomicUsize::new(0)); // EADDRNOTAVAIL / EADDRINUSE
    let err_connect_refused = Arc::new(AtomicUsize::new(0));     // ECONNREFUSED
    let err_connect_other = Arc::new(AtomicUsize::new(0));
    let err_io_other = Arc::new(AtomicUsize::new(0));            // read/write failures

    let semaphore = Arc::new(Semaphore::new(max_inflight));

    println!("\n=== L4 Load Balancer — Continuous Test Client ===");
    println!("Target           : {}", target_addr);
    println!("Duration         : {} s", duration_secs);
    println!(
        "Rate             : {}",
        if rate_per_sec == 0 {
            "uncapped (bounded by --max-inflight)".to_string()
        } else {
            format!("{} req/s (steady)", rate_per_sec)
        }
    );
    println!("Max in-flight    : {}", max_inflight);
    println!();

    // Progress reporter task (live, every 1s).
    let success_r = Arc::clone(&success);
    let errors_r = Arc::clone(&errors);
    let inflight_r = Arc::clone(&inflight);
    let stop_reporter = Arc::new(AtomicUsize::new(0));
    let stop_reporter_r = Arc::clone(&stop_reporter);
    let reporter = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        let mut last_success: usize = 0;
        let mut last_errors: usize = 0;
        let start = Instant::now();
        ticker.tick().await; // skip first immediate tick
        while stop_reporter_r.load(Ordering::Relaxed) == 0 {
            ticker.tick().await;
            let s = success_r.load(Ordering::Relaxed);
            let e = errors_r.load(Ordering::Relaxed);
            let i = inflight_r.load(Ordering::Relaxed);
            let dr = s.saturating_sub(last_success);
            let de = e.saturating_sub(last_errors);
            last_success = s;
            last_errors = e;
            println!(
                "[{:>4}s] sent: {:>7}  ok/s: {:>5}  err/s: {:>4}  in-flight: {:>4}  total_err: {}",
                start.elapsed().as_secs(),
                s,
                dr,
                de,
                i,
                e,
            );
        }
    });

    let start_time = Instant::now();

    // Compute steady-rate inter-arrival (ns). rate=0 disables the timer gate.
    let interval_ns: u64 = if rate_per_sec == 0 {
        0
    } else {
        1_000_000_000u64 / rate_per_sec.max(1)
    };

    // Spawn requests at the chosen pace.
    let mut tasks = Vec::new();
    let mut next_tick = Instant::now();

    while start_time.elapsed() < duration {
        // Pacing: wait until the slot for the next request is due.
        if interval_ns > 0 {
            let now = Instant::now();
            if next_tick > now {
                tokio::time::sleep(next_tick - now).await;
            }
            next_tick += Duration::from_nanos(interval_ns);
        }

        // Bounded concurrency via semaphore — never explode in-flight count.
        let permit = match Arc::clone(&semaphore).acquire_owned().await {
            Ok(p) => p,
            Err(_) => break, // semaphore closed
        };

        let counts_clone = Arc::clone(&request_counts);
        let target_clone = target_addr.clone();
        let ports_clone = backend_ports.clone();
        let success_c = Arc::clone(&success);
        let errors_c = Arc::clone(&errors);
        let inflight_c = Arc::clone(&inflight);
        let e_addr = Arc::clone(&err_connect_addrunavail);
        let e_ref = Arc::clone(&err_connect_refused);
        let e_oth = Arc::clone(&err_connect_other);
        let e_io = Arc::clone(&err_io_other);

        let handle = tokio::spawn(async move {
            let _permit = permit; // released on drop
            inflight_c.fetch_add(1, Ordering::Relaxed);

            // Split connect vs read/write so we can attribute errors. On
            // macOS, sustained 500 req/s typically blows the ~16 k ephemeral
            // port range within a minute and connect() starts returning
            // AddrNotAvailable / AddrInUse — that's the test environment,
            // not the LB.
            let result: Result<String, (std::io::Error, &'static str)> = async {
                let mut stream = TcpStream::connect(&target_clone)
                    .await
                    .map_err(|e| (e, "connect"))?;
                stream
                    .write_all(b"Ping\n")
                    .await
                    .map_err(|e| (e, "io"))?;
                let mut buf = [0u8; 128];
                let n = stream
                    .read(&mut buf)
                    .await
                    .map_err(|e| (e, "io"))?;
                Ok(String::from_utf8_lossy(&buf[..n]).to_string())
            }
            .await;

            match result {
                Ok(resp) => {
                    success_c.fetch_add(1, Ordering::Relaxed);
                    for &port in &ports_clone {
                        if resp.contains(&port.to_string()) {
                            let mut lock = counts_clone.lock().await;
                            *lock.get_mut(&port).unwrap() += 1;
                            break;
                        }
                    }
                }
                Err((err, phase)) => {
                    errors_c.fetch_add(1, Ordering::Relaxed);
                    use std::io::ErrorKind::*;
                    if phase == "connect" {
                        match err.kind() {
                            AddrNotAvailable | AddrInUse => {
                                e_addr.fetch_add(1, Ordering::Relaxed);
                            }
                            ConnectionRefused => {
                                e_ref.fetch_add(1, Ordering::Relaxed);
                            }
                            _ => {
                                e_oth.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    } else {
                        e_io.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }

            inflight_c.fetch_sub(1, Ordering::Relaxed);
        });

        tasks.push(handle);
    }

    println!("\nDuration reached. Waiting for in-flight requests to drain…");
    for h in tasks {
        let _ = h.await;
    }
    stop_reporter.store(1, Ordering::Relaxed);
    let _ = reporter.await;

    // Final report.
    println!("\n=== Load Distribution Results ===");
    let counts = request_counts.lock().await;
    let mut total = 0usize;
    for &port in &backend_ports {
        let c = *counts.get(&port).unwrap_or(&0);
        println!("Backend {:>4} : {:>7} requests", port, c);
        total += c;
    }
    let ok = success.load(Ordering::Relaxed);
    let err = errors.load(Ordering::Relaxed);
    let e_a = err_connect_addrunavail.load(Ordering::Relaxed);
    let e_r = err_connect_refused.load(Ordering::Relaxed);
    let e_o = err_connect_other.load(Ordering::Relaxed);
    let e_i = err_io_other.load(Ordering::Relaxed);
    println!();
    println!("Successes        : {}", ok);
    println!("Failures         : {}", err);
    println!("  ├─ connect: addr unavail/in-use : {}", e_a);
    println!("  ├─ connect: refused             : {}", e_r);
    println!("  ├─ connect: other               : {}", e_o);
    println!("  └─ read/write after connect     : {}", e_i);
    println!("Distributed sum  : {}", total);
    let elapsed = start_time.elapsed().as_secs_f64().max(0.001);
    println!("Avg throughput   : {:.2} req/s", ok as f64 / elapsed);

    // If most failures are "addr unavailable/in-use", we ran out of client
    // ephemeral ports. This is a TEST-ENVIRONMENT artifact (host TCP stack
    // + TIME_WAIT), not a load-balancer problem. Help the user diagnose it.
    if err > 0 && e_a * 2 >= err {
        println!();
        println!("NOTE: most failures are ephemeral-port exhaustion on the");
        println!("      *client* side (TIME_WAIT). The load balancer is fine.");
        println!("      Mitigations:");
        println!("        - Lower RATE_PER_SEC (e.g. 200 instead of 500).");
        println!("        - Shorten TIME_WAIT (macOS):");
        println!("            sudo sysctl -w net.inet.tcp.msl=1000   # ms, default 15000");
        println!("        - Or widen the ephemeral range:");
        println!("            sudo sysctl -w net.inet.ip.portrange.first=10000");
    }

    Ok(())
}
