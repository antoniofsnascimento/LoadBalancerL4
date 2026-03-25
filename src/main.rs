use tokio::net::TcpListener;
use std::sync::Arc;
use load_balancer_l4::models::Backend;
use load_balancer_l4::proxy::handle_connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backends = vec![
        Backend { addr: "127.0.0.1:8081".to_string() },
        Backend { addr: "127.0.0.1:8082".to_string() }
    ];

    let backends = Arc::new(backends);
    let addr = "127.0.0.1:8080";
    let listener = TcpListener::bind(addr).await?;
    println!("Listening on {}", addr);

    loop {
        let (client_stream, client_addr) = listener.accept().await?;
        println!("New connection from {}", client_addr);

        let backend_ref = Arc::clone(&backends);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(client_stream, backend_ref).await {
                eprintln!("Error handling connection: {}", e);
            }
        });
    }
}