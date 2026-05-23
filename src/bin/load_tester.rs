use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Entry point for the load testing client.
/// Simulates a high-concurrency scenario by spawning thousands of async tasks.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let lb_addr = "127.0.0.1:8080";

    // Initializing load test with 10,000 parallel requests.
    let num_requests = 10_000;

    println!("========================================");
    println!("INITIALIZING EXTREME LOAD TEST");
    println!("========================================");
    println!("Target: Load Balancer at {}", lb_addr);
    println!(
        "Firing {} asynchronous connections in parallel...",
        num_requests
    );

    let success_count = Arc::new(AtomicUsize::new(0));
    let error_count = Arc::new(AtomicUsize::new(0));

    let start_time = Instant::now();
    let mut tasks = Vec::new();

    for _ in 0..num_requests {
        let success_clone = Arc::clone(&success_count);
        let error_clone = Arc::clone(&error_count);

        tasks.push(tokio::spawn(async move {
            match TcpStream::connect(lb_addr).await {
                Ok(mut stream) => {
                    // Send a fast ping and immediately close the connection
                    if stream.write_all(b"LoadTestPing\n").await.is_ok() {
                        success_clone.fetch_add(1, Ordering::Relaxed);
                    } else {
                        error_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    error_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    // Wait for all micro-tasks to finish
    for task in tasks {
        let _ = task.await;
    }

    let elapsed = start_time.elapsed();

    println!("\n=== Load Test Results ===");
    println!("Total Time: {:?}", elapsed);
    println!(
        "Successes: {} connections",
        success_count.load(Ordering::Relaxed)
    );
    println!(
        "Failures/Rejections: {} connections",
        error_count.load(Ordering::Relaxed)
    );

    let req_per_sec = (num_requests as f64) / elapsed.as_secs_f64();
    println!("Performance metric: {:.2} connections/second", req_per_sec);
    println!("========================================");

    Ok(())
}
