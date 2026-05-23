# L4 TCP Load Balancer in Rust

A high-performance Layer 4 (Transport) TCP Load Balancer implemented in Rust. This project focuses on memory safety, asynchronous concurrency, and low-latency traffic distribution without relying on a garbage collector.

## Features

- **Asynchronous TCP proxy** built on Tokio with bidirectional copy.
- **Routing algorithms:** Round Robin and Weighted Round Robin, switchable live.
- **Hot-reload control plane** backed by Redis — operators can change weights or the active algorithm at runtime without restarting the load balancer.
- **Health checker (watchdog)** probes each backend over TCP every 2 seconds and publishes the result on Redis. Unhealthy backends are taken out of rotation automatically and brought back when they recover.
- **Live telemetry dashboard** exposed at port `3000`: global throughput (CPS, BPS), active/total connections, per-backend stats, and an interactive view that lets you change weights and switch algorithm from the browser.
- **Containerised** stack (`docker compose up`) with the load balancer, four backends, the watchdog, and Redis.
- **Built-in load test client** for stress-testing the balancer end-to-end.

## Tech Stack

- **Language:** [Rust (Edition 2024)](https://www.rust-lang.org/)
- **Runtime:** [Tokio v1](https://tokio.rs/) (Asynchronous I/O)
- **HTTP layer:** [Axum](https://docs.rs/axum/) (dashboard + control API)
- **Control plane:** [Redis](https://redis.io/) (weights, algorithm, health flags)
- **Layer:** OSI Layer 4 (TCP)

## Project Documentation Map

1. [**Architecture Design**](docs/architecture.md): system structure, concurrency model, control plane, and module organisation.
2. [**TCP Proxy Implementation**](docs/tcpProxy.md): line-by-line walkthrough of the network engine and data routing (historical, from the project's first phase).

## Getting Started

### Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install) (Edition 2024).
- [Docker](https://www.docker.com/) with Docker Compose, if you want to run the full stack.
- A running [Redis](https://redis.io/) instance for the hot-reload control plane (provided automatically by `docker compose`).

### Option A — Full stack with Docker (recommended)

```bash
docker compose up --build
```

This boots:

- `lb` — the load balancer on `:8080` and the dashboard on `:3000`
- `backend1..4` — four dummy TCP backends on `:8081..8084`
- `health_checker` — the watchdog
- `redis` — the shared control plane

Open `http://localhost:3000` to see the live dashboard.

### Option B — Run locally with `cargo`

1. Clone the repository:
   ```bash
   git clone <repository-url>
   cd LoadBalancerL4
   ```
2. Start a Redis instance (e.g. `docker run -p 6379:6379 redis:alpine`).
3. Compile and run the load balancer:
   ```bash
   cargo run --bin load_balancer_l4
   ```
   You'll be prompted to pick the routing algorithm (Round Robin or Weighted Round Robin).
4. In separate terminals, start the helper binaries:
   ```bash
   PORT=8081 cargo run --bin backend
   PORT=8082 cargo run --bin backend
   PORT=8083 cargo run --bin backend
   PORT=8084 cargo run --bin backend
   cargo run --bin health_checker
   ```
5. Open `http://localhost:3000` to see the dashboard.

### Stress-testing

Two load clients are bundled:

```bash
# Fixed-rate sustained traffic (default: 60 s, 500 req/s, 200 in-flight).
cargo run --bin test_client -- 60 500 200

# Burst test: 10 000 parallel connections.
cargo run --bin load_tester
```

## Configuration (environment variables)

| Variable      | Default                                                | Purpose                                          |
|---------------|--------------------------------------------------------|--------------------------------------------------|
| `LISTEN_ADDR` | `127.0.0.1:8080`                                       | Address the load balancer listens on.            |
| `BACKENDS`    | `127.0.0.1:8081,127.0.0.1:8082,127.0.0.1:8083,127.0.0.1:8084` | Comma-separated list of backend `host:port`.     |
| `WEIGHTS`     | `3,1,1,1`                                              | Initial Weighted Round Robin weights.            |
| `ALGORITHM`   | (interactive prompt)                                   | `1`/`RoundRobin` or `2`/`WeightedRoundRobin`.    |
| `REDIS_URL`   | `redis://127.0.0.1:6379/`                              | Control-plane Redis URL.                         |
| `PORT`        | `8081` (in `backend` binary only)                      | Port the dummy backend listens on.               |

## Running the tests

```bash
cargo test
```
