use tokio::net::TcpListener;
use std::sync::Arc;
use std::io::{self, Write};
use std::sync::atomic::Ordering;
use load_balancer_l4::models::Backend;
use load_balancer_l4::balancer::{Algorithm, LoadBalancer};
use load_balancer_l4::proxy::handle_connection;
use load_balancer_l4::telemetry::Telemetry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Limpar a consola (usando códigos ANSI)
    print!("{}[2J{}[1;1H", 27 as char, 27 as char);
    
    println!("=== Load Balancer L4 ===");
    println!("Escolha o algoritmo de distribuição:");
    println!("1) Round Robin Clássico (Distribuição 1:1)");
    println!("2) Weighted Round Robin (Pesos 3, 1, 1, 1)");
    print!("> ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    
    let algorithm = match input.trim() {
        "1" => Algorithm::RoundRobin,
        "2" => Algorithm::WeightedRoundRobin,
        _ => {
            println!("Opção inválida, a usar Weighted Round Robin por defeito.");
            Algorithm::WeightedRoundRobin
        }
    };
    
    println!("Algoritmo selecionado: {:?}\n", algorithm);
    let backends = vec![
        Backend { addr: "127.0.0.1:8081".to_string(), weight: 3 }, // 3 requests seguidos ou interpolados
        Backend { addr: "127.0.0.1:8082".to_string(), weight: 1 },
        Backend { addr: "127.0.0.1:8083".to_string(), weight: 1 },
        Backend { addr: "127.0.0.1:8084".to_string(), weight: 1 }
    ];

    // Create the LoadBalancer and wrap it in an Arc to share between tasks
    let balancer = Arc::new(LoadBalancer::new(backends, algorithm));
    let telemetry = Arc::new(Telemetry::new(balancer.backends().len()));

    let tel_clone = Arc::clone(&telemetry);
    let balancer_clone = Arc::clone(&balancer);
    
    // TUI Loop (Dashboard estilo BIOS em background)
    tokio::spawn(async move {
        // Limpar o ecrã apenas uma vez no início
        print!("{}[2J{}[1;1H", 27 as char, 27 as char);
        let mut first_render = true;
        
        loop {
            if !first_render {
                // Move o cursor 16 linhas para cima para reescrever a tabela no mesmo sítio
                print!("{}[16A", 27 as char);
            }
            first_render = false;
            
            println!("---------------------------------------------------------------");
            println!("              LOAD BALANCER L4 - TELEMETRIA BIOS               ");
            println!("---------------------------------------------------------------");
            println!("Algoritmo Ativo: {:?}", balancer_clone.algorithm());
            println!("Conexões Ativas (Fila/Buffer): {}", tel_clone.total_active_connections.load(Ordering::Relaxed));
            println!("Total de Ligações Históricas: {}", tel_clone.total_connections_accepted.load(Ordering::Relaxed));
            println!("Volume de Dados Transferidos: {} Bytes", tel_clone.total_bytes_transferred.load(Ordering::Relaxed));
            println!("\n[Estado dos Servidores / Backends]");
            println!("{:<16} | {:<4} | {:<6} | {:<7} | {:<12}", "IP:Porta", "Peso", "Ativas", "Total", "Tráfego (B)");
            println!("---------------------------------------------------------------");
            
            for (id, backend) in balancer_clone.backends().iter().enumerate() {
                let stats = &tel_clone.backend_stats[id];
                let active = stats.active_connections.load(Ordering::Relaxed);
                let total = stats.total_connections.load(Ordering::Relaxed);
                let bytes = stats.bytes_transferred.load(Ordering::Relaxed);
                println!("{:<16} | {:<4} | {:<6} | {:<7} | {:<12}", backend.addr, backend.weight, active, total, bytes);
            }
            println!("---------------------------------------------------------------");
            
            // Taxa de atualização (10fps)
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    });

    let addr = "127.0.0.1:8080";
    let listener = TcpListener::bind(addr).await?;

    loop {
        let (client_stream, _client_addr) = listener.accept().await?;

        let backend_ref = Arc::clone(&balancer);
        let tel_ref = Arc::clone(&telemetry);
        
        tokio::spawn(async move {
            let _ = handle_connection(client_stream, backend_ref, tel_ref).await;
        });
    }
}