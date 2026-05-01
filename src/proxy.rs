use tokio::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::balancer::LoadBalancer;
use crate::telemetry::Telemetry;

pub async fn handle_connection(
    mut client_stream: TcpStream,
    balancer: Arc<LoadBalancer>,
    telemetry: Arc<Telemetry>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Marcar conexão como recebida (Na Fila do LB)
    telemetry.total_active_connections.fetch_add(1, Ordering::Relaxed);
    telemetry.total_connections_accepted.fetch_add(1, Ordering::Relaxed);

    let (backend_id, backend) = balancer.next_backend();
    let target_addr = &backend.addr;
    
    // 2. Marcar conexão como encaminhada para o Backend escolhido
    telemetry.backend_stats[backend_id].active_connections.fetch_add(1, Ordering::Relaxed);
    telemetry.backend_stats[backend_id].total_connections.fetch_add(1, Ordering::Relaxed);
    
    // Encapsular o streaming de rede para garantirmos a limpeza do estado no final independentemente de dar erro
    let result = async {
        let mut backend_stream = TcpStream::connect(target_addr).await?;
        let (from_client, from_backend) = tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream).await?;
        
        let total_bytes = from_client + from_backend;
        telemetry.total_bytes_transferred.fetch_add(total_bytes as usize, Ordering::Relaxed);
        telemetry.backend_stats[backend_id].bytes_transferred.fetch_add(total_bytes as usize, Ordering::Relaxed);
        
        Ok::<(), Box<dyn std::error::Error>>(())
    }.await;
    
    // 3. Limpar a conexão ativa das filas (Subtrair de ambos) quando a transferência terminar
    telemetry.total_active_connections.fetch_sub(1, Ordering::Relaxed);
    telemetry.backend_stats[backend_id].active_connections.fetch_sub(1, Ordering::Relaxed);
    
    result
}
