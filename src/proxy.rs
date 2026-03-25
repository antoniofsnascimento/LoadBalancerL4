use tokio::net::TcpStream;
use std::sync::Arc;
use crate::models::Backend;

pub async fn handle_connection(
    mut client_stream: TcpStream,
    backends: Arc<Vec<Backend>>
) -> Result<(), Box<dyn std::error::Error>> {
    let target_addr = &backends[0].addr;
    let mut backend_stream = TcpStream::connect(target_addr).await?;
    println!("Connected to backend {}", target_addr);

    let (from_client, _from_backend) = tokio::io::copy_bidirectional(
        &mut client_stream,
        &mut backend_stream
    ).await?;

    println!("From client: {}", from_client);
    Ok(())
}