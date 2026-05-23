use crate::balancer::LoadBalancer;
use crate::telemetry::Telemetry;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::net::TcpStream;

pub async fn handle_connection(
    mut client_stream: TcpStream,
    balancer: Arc<LoadBalancer>,
    telemetry: Arc<Telemetry>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Mark connection as received (in LB queue)
    telemetry
        .total_active_connections
        .fetch_add(1, Ordering::Relaxed);
    telemetry
        .total_connections_accepted
        .fetch_add(1, Ordering::Relaxed);

    let (backend_id, backend) = balancer.next_backend();
    let target_addr = backend.addr.clone();

    // 2. Mark connection as forwarded to the chosen backend
    telemetry.backend_stats[backend_id]
        .active_connections
        .fetch_add(1, Ordering::Relaxed);
    telemetry.backend_stats[backend_id]
        .total_connections
        .fetch_add(1, Ordering::Relaxed);

    // Encapsulate network streaming to ensure state cleanup at the end, regardless of errors
    let result = async {
        let mut backend_stream = TcpStream::connect(target_addr).await?;
        let (from_client, from_backend) =
            tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream).await?;

        let total_bytes = from_client + from_backend;
        telemetry
            .total_bytes_transferred
            .fetch_add(total_bytes as usize, Ordering::Relaxed);
        telemetry.backend_stats[backend_id]
            .bytes_transferred
            .fetch_add(total_bytes as usize, Ordering::Relaxed);

        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    // 3. Clear the active connection from the queues when the transfer finishes
    telemetry
        .total_active_connections
        .fetch_sub(1, Ordering::Relaxed);
    telemetry.backend_stats[backend_id]
        .active_connections
        .fetch_sub(1, Ordering::Relaxed);

    result
}
