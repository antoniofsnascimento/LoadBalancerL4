use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Entry point for the backend server.
/// Listens on the port specified by the `PORT` environment variable.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8081".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    println!("Backend server started on {}", addr);

    loop {
        if let Ok((mut socket, _client_addr)) = listener.accept().await {
            let port_clone = port.clone();

            tokio::spawn(async move {
                let mut buf = [0; 1024];
                if let Ok(n) = socket.read(&mut buf).await {
                    if n > 0 {
                        let _request = String::from_utf8_lossy(&buf[..n]);
                    }
                }

                let message = format!("Hello from Dockerized Backend {}!\n", port_clone);
                let _ = socket.write_all(message.as_bytes()).await;
            });
        }
    }
}
