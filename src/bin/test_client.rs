use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend_ports = vec![8081, 8082, 8083, 8084];
    let lb_port = 8080;
    
    // Teste para o Weighted Round Robin (Pesos: 3, 1, 1, 1 -> total de 6)
    // 60 pedidos devem resultar em: 8081 (30), restantes (10 cada).
    let num_requests = 60; 

    // Mapa para registar quantos pedidos cada porto recebeu
    let request_counts = Arc::new(Mutex::new(HashMap::new()));
    for &port in &backend_ports {
        request_counts.lock().await.insert(port, 0);
    }

    // 1. Iniciar os "Falsos Backends" localmente
    for &port in &backend_ports {
        let counts = Arc::clone(&request_counts);
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await.unwrap();
            println!("Backend started on port {}", port);
            
            loop {
                if let Ok((mut socket, _addr)) = listener.accept().await {
                    let mut counts_lock = counts.lock().await;
                    *counts_lock.get_mut(&port).unwrap() += 1;
                    
                    let message = format!("Hello from backend {}!\n", port);
                    let _ = socket.write_all(message.as_bytes()).await;
                }
            }
        });
    }

    // Dar uns milissegundos para os listeners iniciarem corretamente
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    println!("\nStarting load balancer test...");
    println!("Sending {} requests to Load Balancer on port {}...\n", num_requests, lb_port);

    // 2. Disparar os pedidos em massa contra o Load Balancer
    for i in 1..=num_requests {
        match TcpStream::connect(format!("127.0.0.1:{}", lb_port)).await {
            Ok(mut stream) => {
                let _ = stream.write_all(b"Ping\n").await;
                let mut buf = [0; 100];
                if let Ok(n) = stream.read(&mut buf).await {
                    let response = String::from_utf8_lossy(&buf[..n]);
                    println!("[Client] Request {:02} response: {}", i, response.trim());
                }
            }
            Err(e) => println!("[Client] Request {:02} failed: {}", i, e),
        }
        
        // Pequena pausa para não esgotar as sockets instantaneamente e para ser legível no ecrã
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // 3. Imprimir o relatório final de telemetria base
    println!("\n=== Load Distribution Results (WRR) ===");
    let counts = request_counts.lock().await;
    for &port in &backend_ports {
        println!("Backend {}: {} requests", port, counts.get(&port).unwrap());
    }

    Ok(())
}
