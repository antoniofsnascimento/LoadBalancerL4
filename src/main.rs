use load_balancer_l4::balancer::{Algorithm, LoadBalancer}; // Imports Algorithm (enum with RR and WRR) and LoadBalancer (structure managing the backend pool) from the balancer module
use load_balancer_l4::models::Backend; // Imports the Backend struct, which represents a backend server with its address and weight
use load_balancer_l4::proxy::handle_connection; // The core of L4 proxy engine: imports this function to spwan it when a new TCP connection arrives
use load_balancer_l4::telemetry::Telemetry; // Central metrics scoreboard: imports the Telemetry struct that holds all the counters and stats for the dashboard
use std::io::{self, Write}; // For interactive menu input and output flushing (ensures prompt appears before input)
use std::sync::Arc; // For thread-safe reference counting of shared state (LoadBalancer and Telemetry) across async tasks
use std::sync::atomic::Ordering; // For specifying memory ordering when updating atomic counters in Telemetry (Relaxed is sufficient for our use case)
use tokio::net::TcpListener; // tokio provides async TPC sockets for the actual load balancer
    
use axum::{ // builds the HTTP dashboard API
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

/// Defensive upper bound for operator-supplied weights. Above ~20 the practical
/// effect on the distribution is invisible, so 100 is plenty of headroom.
const MAX_WEIGHT: u32 = 100; // This is a sanity check to prevent a UI bug or a misguided script from

#[derive(Serialize)] // For serializing the telemetry response to JSON for the dashboard
struct GlobalStats { // Aggregated metrics across all backends, shown in the dashboard header
    active_connections: usize,
    total_connections: usize,
    bytes_transferred: usize,
}

#[derive(Serialize)] // For serializing individual backend data in the telemetry response
struct BackendStatData { // Metrics specific to each backend, shown in the dashboard's backend table
    active_connections: usize,
    total_connections: usize,
    bytes_transferred: usize,
}

#[derive(Serialize)] // For serializing the backend data in the telemetry response  
struct BackendData { // Represents the state of each backend as shown in the dashboard, combining static info (address) with dynamic state (weights, health, stats)
    addr: String,
    /// Effective routing weight (what build_virtual_indices used).
    /// Equal to user_weight × health. 0 means out of rotation.
    weight: u32,
    /// What the operator set in Redis (`weight:<addr>`). Survives health flaps
    /// so the UI can show "manually disabled (0)" vs "health-down".
    intent_weight: u32,
    /// Reported by the watchdog (`health:<addr>`). true=reachable, false=down.
    healthy: bool,
    stats: BackendStatData,
}

#[derive(Serialize)] // For serializing the entire telemetry response, which includes global stats and a list of backends, for the dashboard API
struct TelemetryResponse { // The full payload returned by GET /api/telemetry, which the dashboard consumes to display the current state of the load balancer and its backends
    algorithm: String,
    global: GlobalStats,
    backends: Vec<BackendData>,
}

/// Latest snapshot of what was read from Redis for each backend. The Redis
/// polling task writes this; the HTTP handler reads it so the dashboard can
/// show **intent** (operator weight) and **health** separately from the
/// **effective** weight that the balancer actually uses for routing.
#[derive(Default, Clone)]
struct BackendRedisState {
    intent_weight: u32,
    healthy: bool,
}

#[derive(Clone)]
struct AppState {
    balancer: Arc<LoadBalancer>,
    telemetry: Arc<Telemetry>,
    redis_state: Arc<std::sync::RwLock<Vec<BackendRedisState>>>,
    /// Shared Redis client. Each handler asks it for a multiplexed connection
    /// per request; the client itself is just a parsed URL, cheap to clone.
    redis_client: Arc<redis::Client>,
    /// Snapshot of the WEIGHTS env at startup, used by POST /api/reset to
    /// return the system to its known-good baseline.
    default_weights: Arc<Vec<u32>>,
    /// Algorithm chosen at startup (from the ALGORITHM env or the menu);
    /// also restored by POST /api/reset.
    default_algorithm: Algorithm,
}

async fn serve_dashboard() -> Html<&'static str> { // Serves the static HTML dashboard. The HTML file is included at compile time, so this is a zero-cost operation that doesn't hit the filesystem at runtime. 
    Html(include_str!("dashboard.html"))
}

async fn health() -> &'static str { // Simple health check endpoint for Kubernetes or load testing tools. Always returns 200 OK with a plain "ok" body, since this LB doesn't have any internal state that would cause it to be unhealthy.
    "ok" 
}

async fn get_telemetry(State(state): State<AppState>) -> Json<TelemetryResponse> { // Handler for GET /api/telemetry, which the dashboard calls to get the current state of the load balancer and its backends. It reads from the shared Telemetry and Redis state to construct a snapshot of the global stats and per-backend data, then returns it as JSON.
    let global = GlobalStats { 
        active_connections: state
            .telemetry
            .total_active_connections
            .load(Ordering::Relaxed),
        total_connections: state
            .telemetry
            .total_connections_accepted
            .load(Ordering::Relaxed),
        bytes_transferred: state
            .telemetry
            .total_bytes_transferred
            .load(Ordering::Relaxed),
    };

    let mut backends = Vec::new(); // Build the list of backends for the response by combining data from the LoadBalancer (static info and effective weights) with the latest snapshot from Redis (intent weights and health) and the Telemetry stats. This allows the dashboard to show a comprehensive view of each backend's state.
    let redis_snapshot = state.redis_state.read().unwrap().clone();
    for (id, backend) in state.balancer.backends().iter().enumerate() {
        let stats = &state.telemetry.backend_stats[id];
        let rs = redis_snapshot.get(id).cloned().unwrap_or_default();
        backends.push(BackendData { // Construct the BackendData for this backend, which includes:
            addr: backend.addr.clone(),
            weight: backend.weight, // effective (already intent×health)
            intent_weight: rs.intent_weight,
            healthy: rs.healthy,
            stats: BackendStatData { // Convert atomic counters to plain usize for the response
                active_connections: stats.active_connections.load(Ordering::Relaxed),
                total_connections: stats.total_connections.load(Ordering::Relaxed),
                bytes_transferred: stats.bytes_transferred.load(Ordering::Relaxed),
            },
        });
    }

    Json(TelemetryResponse {
        algorithm: format!("{:?}", state.balancer.algorithm()),
        global,
        backends,
    })
}

// ---------------------------------------------------------------------------
// Mutating HTTP endpoints — let the dashboard reconfigure the LB without
// touching redis-cli. They all write to Redis only; the polling loop
// propagates the change into the live balancer (≤1 s).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SetWeightReq {
    addr: String,
    weight: u32,
}

#[derive(Deserialize)]
struct SetAlgorithmReq {
    algorithm: String,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { error: msg.into() }))
}

async fn set_weight(
    State(state): State<AppState>,
    Json(req): Json<SetWeightReq>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    // Validate weight bound.
    if req.weight > MAX_WEIGHT {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!("weight must be 0..={}", MAX_WEIGHT),
        ));
    }
    // Validate addr — must be one of the loaded backends. This stops the UI
    // (or a misguided script) from polluting Redis with stale `weight:foo`
    // keys for non-existent nodes.
    let known: bool = state.balancer.backends().iter().any(|b| b.addr == req.addr); // Check if the provided address matches any of the known backends in the LoadBalancer. This is a safeguard to prevent setting weights for unknown backends, which could indicate a typo in the dashboard or an attempt to manipulate Redis directly with invalid keys. If the address is not recognized, we return a 404 Not Found error to the dashboard.
    if !known {
        return Err(err(StatusCode::NOT_FOUND, format!("unknown backend {}", req.addr)));
    }

    let mut con = state // Get a multiplexed connection from the shared Redis client. This is an async operation that may fail if Redis is unavailable, in which case we return a 503 Service Unavailable error to the dashboard.
        .redis_client
        .get_multiplexed_tokio_connection()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("redis: {e}")))?;

    let key = format!("weight:{}", req.addr); // Construct the Redis key for this backend's weight based on its address. This is the key that the polling loop watches to update the balancer's state.
    let _: () = redis::cmd("SET") // Issue the Redis command to set the new weight for this backend. The polling loop will pick up this change and update the LoadBalancer's effective weights on the next iteration (within 1 second). If the Redis command fails, we return a 503 Service Unavailable error to the dashboard.
        .arg(&key)
        .arg(req.weight)
        .query_async(&mut con)
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("redis SET: {e}")))?;

    Ok(StatusCode::NO_CONTENT) // Return 204 No Content on success, since the dashboard doesn't need any data back from this request.
}

async fn set_algorithm( // Handler for POST /api/algorithm, which allows the dashboard to change the load balancing algorithm at runtime. It writes the new algorithm to Redis, and the polling loop will detect this change and update the LoadBalancer's algorithm accordingly. This decouples the HTTP API from the balancer's internal state management, ensuring thread safety and consistency.
    State(state): State<AppState>,
    Json(req): Json<SetAlgorithmReq>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let alg = Algorithm::parse(&req.algorithm).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            format!("invalid algorithm: {}", req.algorithm),
        )
    })?;

    let mut con = state
        .redis_client
        .get_multiplexed_tokio_connection()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("redis: {e}")))?;

    // Write the canonical Debug form so the polling loop's Algorithm::parse
    // recognises it without ambiguity.
    let alg_str = format!("{:?}", alg);
    let _: () = redis::cmd("SET")
        .arg("lb:algorithm")
        .arg(&alg_str)
        .query_async(&mut con)
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("redis SET: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn reset_defaults(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut con = state
        .redis_client
        .get_multiplexed_tokio_connection()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("redis: {e}")))?;

    let backends = state.balancer.backends();
    for (i, backend) in backends.iter().enumerate() {
        let default_w = state.default_weights.get(i).copied().unwrap_or(1);
        let w_key = format!("weight:{}", backend.addr);
        let wrr_key = format!("wrr_weight:{}", backend.addr);
        let _: Result<(), _> = redis::cmd("SET").arg(&w_key).arg(default_w).query_async(&mut con).await;
        // Drop the WRR snapshot so the next algorithm flip recomputes from
        // the current (now reset) state.
        let _: Result<(), _> = redis::cmd("DEL").arg(&wrr_key).query_async(&mut con).await;
    }

    let alg_str = format!("{:?}", state.default_algorithm);
    let _: Result<(), _> = redis::cmd("SET")
        .arg("lb:algorithm")
        .arg(&alg_str)
        .query_async(&mut con)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Docker Support (bypass interactive menu using Environment Variables)
    let algorithm = if let Ok(algo_env) = std::env::var("ALGORITHM") {
        match algo_env.as_str() {
            "1" | "RoundRobin" => Algorithm::RoundRobin,
            "2" | "WeightedRoundRobin" => Algorithm::WeightedRoundRobin,
            _ => Algorithm::WeightedRoundRobin,
        }
    } else {
        // Clear console and show menu if running locally
        print!("{}[2J{}[1;1H", 27 as char, 27 as char);
        println!("=== Load Balancer L4 ===");
        println!("Choose the distribution algorithm:");
        println!("1) Classic Round Robin (1:1 Distribution)");
        println!("2) Weighted Round Robin (Weights 3, 1, 1, 1)");
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        match input.trim() {
            "1" => Algorithm::RoundRobin,
            "2" => Algorithm::WeightedRoundRobin,
            _ => {
                println!("Invalid option, defaulting to Weighted Round Robin.");
                Algorithm::WeightedRoundRobin
            }
        }
    };

    println!("Selected algorithm: {:?}\n", algorithm);

    // 2. Resolve Backend Server Addresses (Local vs Docker)
    let backends_env = std::env::var("BACKENDS").unwrap_or_else(|_| {
        "127.0.0.1:8081,127.0.0.1:8082,127.0.0.1:8083,127.0.0.1:8084".to_string()
    });

    // Parse WEIGHTS env (comma-separated). Falls back to 3:1:1:1, then 1 for extras.
    let weights: Vec<u32> = std::env::var("WEIGHTS")
        .unwrap_or_else(|_| "3,1,1,1".to_string())
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();

    let mut backends = Vec::new();
    for (i, addr) in backends_env.split(',').enumerate() {
        let weight = weights.get(i).copied().unwrap_or(1);
        backends.push(Backend {
            addr: addr.trim().to_string(),
            weight,
        });
    }

    // Create the LoadBalancer and wrap it in an Arc to share between tasks
    let balancer = Arc::new(LoadBalancer::new(backends, algorithm));
    let telemetry = Arc::new(Telemetry::new(balancer.backends().len()));
    let redis_state: Arc<std::sync::RwLock<Vec<BackendRedisState>>> = Arc::new(
        std::sync::RwLock::new(vec![BackendRedisState::default(); balancer.backends().len()]),
    );

    // Build the shared Redis client (parses URL only — actual sockets are
    // opened lazily by each consumer).
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
    let redis_client = Arc::new(redis::Client::open(redis_url.clone())?);

    // Stash defaults for POST /api/reset. Pad with 1s if BACKENDS has more
    // entries than WEIGHTS (matches the seeding logic above).
    let mut default_weights = Vec::with_capacity(balancer.backends().len());
    for i in 0..balancer.backends().len() {
        default_weights.push(weights.get(i).copied().unwrap_or(1));
    }
    let default_weights = Arc::new(default_weights);
    let default_algorithm = algorithm;

    // ---------------------------------------------------------------------
    // Background Worker: Redis polling.
    //
    // This is the SINGLE point of truth that bridges Redis → in-memory state.
    // It owns three concerns:
    //   (1) seed control keys at startup (idempotent)
    //   (2) detect algorithm transitions (WRR↔RR) and save/restore weights
    //   (3) compute effective weight = operator_weight × health, and push it
    //       into the balancer
    // ---------------------------------------------------------------------
    let balancer_for_redis = Arc::clone(&balancer);
    let redis_state_for_loop = Arc::clone(&redis_state);
    let redis_client_for_loop = Arc::clone(&redis_client);
    tokio::spawn(async move {
        let client = redis_client_for_loop;

        loop {
            // Multiplexed connection is optimized for tokio
            let mut con = match client.get_multiplexed_tokio_connection().await {
                Ok(c) => c,
                Err(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                    continue;
                }
            };

            // ---- Startup seeding ------------------------------------------
            let backends = balancer_for_redis.backends();

            // (1) Per-backend operator weights — only if absent, preserving
            //     state across LB restarts.
            for backend in backends.iter() {
                let key = format!("weight:{}", backend.addr);
                let exists: bool = redis::cmd("EXISTS")
                    .arg(&key)
                    .query_async(&mut con)
                    .await
                    .unwrap_or(false);
                if !exists {
                    let _: Result<(), _> = redis::cmd("SET")
                        .arg(&key)
                        .arg(backend.weight)
                        .query_async(&mut con)
                        .await;
                }
            }

            // (2) lb:backends — always rebuilt to match the loaded set.
            let _: Result<(), _> = redis::cmd("DEL")
                .arg("lb:backends")
                .query_async(&mut con)
                .await;
            for backend in backends.iter() {
                let _: Result<(), _> = redis::cmd("RPUSH")
                    .arg("lb:backends")
                    .arg(&backend.addr)
                    .query_async(&mut con)
                    .await;
            }

            // (3) lb:algorithm — seeded only if absent.
            let alg_exists: bool = redis::cmd("EXISTS")
                .arg("lb:algorithm")
                .query_async(&mut con)
                .await
                .unwrap_or(false);
            if !alg_exists {
                let alg_str = format!("{:?}", balancer_for_redis.algorithm());
                let _: Result<(), _> = redis::cmd("SET")
                    .arg("lb:algorithm")
                    .arg(&alg_str)
                    .query_async(&mut con)
                    .await;
            }

            // Track the algorithm we last applied so we can detect transitions.
            let mut last_alg = balancer_for_redis.algorithm();

            // ---- Hot-reload poll loop -------------------------------------
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

                let current_backends = balancer_for_redis.backends();

                // -- (A) Algorithm: detect & apply transitions --------------
                // WRR → RR: snapshot current operator weights into
                //          wrr_weight:<addr>, then flatten weight:<addr> to 1
                //          so the UI shows the algorithm-correct value.
                // RR  → WRR: restore weight:<addr> from wrr_weight:<addr>
                //           (fall back to "1" if no snapshot exists).
                let alg_redis: Result<String, _> = redis::cmd("GET")
                    .arg("lb:algorithm")
                    .query_async(&mut con)
                    .await;
                if let Ok(s) = alg_redis {
                    if let Some(new_alg) = Algorithm::parse(&s) {
                        if new_alg != last_alg {
                            match (last_alg, new_alg) {
                                (Algorithm::WeightedRoundRobin, Algorithm::RoundRobin) => {
                                    for backend in current_backends.iter() {
                                        let w_key = format!("weight:{}", backend.addr);
                                        let wrr_key = format!("wrr_weight:{}", backend.addr);
                                        let cur: Result<u32, _> = redis::cmd("GET")
                                            .arg(&w_key)
                                            .query_async(&mut con)
                                            .await;
                                        let to_save = cur.unwrap_or(backend.weight);
                                        let _: Result<(), _> = redis::cmd("SET")
                                            .arg(&wrr_key)
                                            .arg(to_save)
                                            .query_async(&mut con)
                                            .await;
                                        let _: Result<(), _> = redis::cmd("SET")
                                            .arg(&w_key)
                                            .arg(1u32)
                                            .query_async(&mut con)
                                            .await;
                                    }
                                }
                                (Algorithm::RoundRobin, Algorithm::WeightedRoundRobin) => {
                                    for backend in current_backends.iter() {
                                        let w_key = format!("weight:{}", backend.addr);
                                        let wrr_key = format!("wrr_weight:{}", backend.addr);
                                        let saved: Result<u32, _> = redis::cmd("GET")
                                            .arg(&wrr_key)
                                            .query_async(&mut con)
                                            .await;
                                        let to_restore = saved.unwrap_or(1);
                                        let _: Result<(), _> = redis::cmd("SET")
                                            .arg(&w_key)
                                            .arg(to_restore)
                                            .query_async(&mut con)
                                            .await;
                                    }
                                }
                                _ => {}
                            }
                            balancer_for_redis.set_algorithm(new_alg);
                            last_alg = new_alg;
                        }
                    }
                }

                // -- (B) Per-backend operator weight + health → effective ---
                let mut effective_weights = Vec::with_capacity(current_backends.len());
                let mut snapshot = Vec::with_capacity(current_backends.len());
                for backend in current_backends.iter() {
                    let w_key = format!("weight:{}", backend.addr);
                    let h_key = format!("health:{}", backend.addr);

                    let intent: u32 = redis::cmd("GET")
                        .arg(&w_key)
                        .query_async(&mut con)
                        .await
                        .unwrap_or(backend.weight);

                    // Default health to true so that, if the watchdog hasn't
                    // posted yet, we don't blackhole all backends on startup.
                    let healthy: u8 = redis::cmd("GET")
                        .arg(&h_key)
                        .query_async(&mut con)
                        .await
                        .unwrap_or(1);
                    let healthy = healthy != 0;

                    let eff = if healthy { intent } else { 0 };
                    effective_weights.push(eff);
                    snapshot.push(BackendRedisState { intent_weight: intent, healthy });
                }

                balancer_for_redis.update_weights(effective_weights);
                if let Ok(mut guard) = redis_state_for_loop.write() {
                    *guard = snapshot;
                }
            }
        }
    });

    // Launch Web Dashboard on a secondary port
    let state = AppState {
        balancer: Arc::clone(&balancer),
        telemetry: Arc::clone(&telemetry),
        redis_state: Arc::clone(&redis_state),
        redis_client: Arc::clone(&redis_client),
        default_weights: Arc::clone(&default_weights),
        default_algorithm,
    };

    let app = Router::new()
        .route("/", get(serve_dashboard))
        .route("/health", get(health))
        .route("/api/telemetry", get(get_telemetry))
        .route("/api/weight", post(set_weight))
        .route("/api/algorithm", post(set_algorithm))
        .route("/api/reset", post(reset_defaults))
        .with_state(state);

    tokio::spawn(async move {
        let listener = TcpListener::bind("0.0.0.0:3000").await.unwrap();
        println!("Web Dashboard running at http://localhost:3000");
        axum::serve(listener, app).await.unwrap();
    });

    // 3. Load Balancer Listener (with graceful shutdown on Ctrl+C / SIGTERM)
    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = TcpListener::bind(&addr).await?;
    println!("Load Balancer listening on {}", addr);
    println!("Press Ctrl+C to shut down gracefully.\n");

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (client_stream, _client_addr) = match accept_result {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("accept() failed: {e}");
                        continue;
                    }
                };

                let backend_ref = Arc::clone(&balancer);
                let tel_ref = Arc::clone(&telemetry);

                tokio::spawn(async move {
                    let _ = handle_connection(client_stream, backend_ref, tel_ref).await;
                });
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\nShutdown signal received. Stopping accept loop…");
                break;
            }
        }
    }

    println!("Load Balancer stopped. Bye.");
    Ok(())
}
