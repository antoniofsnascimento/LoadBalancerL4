# Architecture Documentation - L4 Load Balancer

This document outlines the fundamental systems engineering decisions made during the development of this load balancer.

## Section 1: Design Decisions

### Asynchronous Concurrency Model (Tokio)

The system operates under a non-blocking I/O model using the [Tokio](https://tokio.rs/) runtime. Unlike traditional OS-thread architectures (one thread per connection), this system utilizes lightweight asynchronous tasks scheduled over a small thread pool (M:N scheduling).

- **Justification:** This approach minimizes context-switching overhead and memory consumption per active connection, allowing the load balancer to handle thousands of concurrent connections effortlessly.

### Layer 4 (Transport) Operation

The load balancer operates strictly at the TCP level. Forwarding decisions are made based on network metadata (IPs and Ports) before any application-level data (Layer 7, like HTTP) is parsed.

- **Advantages:** Unmatched processing latency reduction and protocol versatility (supports any service running over TCP).

### Memory Management and Safety

The network engine leverages Rust's ownership and type system. It guarantees that network buffers and file descriptors are immediately freed upon connection closure, completely preventing memory leaks and data races without a Garbage Collector.

### Routing Algorithms

Two strategies are supported and can be switched live:

- **Round Robin** — every healthy backend (with `weight > 0`) appears once in a flat virtual table; the balancer picks the next entry on each connection.
- **Weighted Round Robin** — each backend appears in the virtual table `weight` times. With `WEIGHTS=3,1,1,1` the first backend takes ~50 % of traffic and the remaining three split the rest evenly.

The virtual-index table is rebuilt on the fly whenever weights or the algorithm change. A safety fallback rotates over **all** backends when every weight is 0, so the LB never panics on `index % 0`.

### Control Plane (Redis)

The load balancer and the watchdog are completely decoupled and communicate **only** through Redis keys:

- `weight:<addr>` — operator-set weight, owned by the dashboard / `redis-cli`.
- `health:<addr>` — 1/0, written **only** by the watchdog.
- `lb:algorithm` — the active algorithm name.
- `lb:backends` — the live backend list (rebuilt at startup).
- `wrr_weight:<addr>` — snapshot of WRR weights taken when switching to Round Robin, so the original weights can be restored on the way back.

A 1 s polling loop inside the load balancer reads these keys and applies the change:

```
effective_weight = operator_weight × health
```

This split fixes two real bugs that an earlier design had:

- Setting `weight:<addr>` to 0 used to be overwritten by the watchdog whenever the next probe succeeded — now it sticks because the watchdog never writes to `weight:*`.
- A backend that is healthy but operator-disabled (weight 0) still reports `health=1`, so re-enabling it puts it back in rotation immediately, with no wait for the next health probe.

### Health Checking (Watchdog)

`health_checker` is a separate binary. Every 2 seconds it tries to open a TCP connection (500 ms timeout) to each backend. Result is published to `health:<addr>`. State transitions (UP→DOWN, DOWN→UP) are logged once; steady-state probes stay silent.

## Section 2: Project Structure

To maintain scalability, the codebase avoids a monolithic `main.rs` file. The logic is split into a small library and a handful of binaries:

### Library ([`src/lib.rs`](../src/lib.rs))

- [`src/balancer.rs`](../src/balancer.rs): `LoadBalancer` and the `Algorithm` enum. Holds the backend list, the virtual-index table, and the live counter. Lock-free read path on the hot loop; only weight/algorithm changes take the write lock.
- [`src/models.rs`](../src/models.rs): `Backend` struct (address + weight).
- [`src/proxy.rs`](../src/proxy.rs): `handle_connection` — picks a backend, opens the upstream TCP stream, runs the bidirectional copy, and updates telemetry counters.
- [`src/telemetry.rs`](../src/telemetry.rs): Atomic counters for global and per-backend stats (active connections, total connections, bytes transferred).

### Binaries

- [`src/main.rs`](../src/main.rs) (`load_balancer_l4`): Entry point. Spawns the Redis polling task, the Axum dashboard server, and the TCP accept loop with graceful Ctrl+C handling.
- [`src/bin/backend.rs`](../src/bin/backend.rs): Tiny TCP echo backend used for local testing.
- [`src/bin/health_checker.rs`](../src/bin/health_checker.rs): The watchdog described above.
- [`src/bin/test_client.rs`](../src/bin/test_client.rs): Steady-rate, semaphore-bounded stress client. Categorises connect / I/O errors so it's easy to spot client-side ephemeral-port exhaustion vs LB problems.
- [`src/bin/load_tester.rs`](../src/bin/load_tester.rs): Burst test — fires 10 000 concurrent connections and measures throughput.

### Web dashboard

- [`src/dashboard.html`](../src/dashboard.html): Embedded with `include_str!`, served by Axum at `/`.
  - `GET  /api/telemetry` — current snapshot (used to redraw at 500 ms).
  - `POST /api/weight` — change a backend's weight.
  - `POST /api/algorithm` — switch routing strategy.
  - `POST /api/reset` — restore the startup defaults.
  - `GET  /health` — liveness probe.

### Containerisation

- [`Dockerfile`](../Dockerfile): Multi-stage build. Stage 1 compiles all binaries with `cargo build --release`. Stage 2 is `debian:bookworm-slim` with only the binaries copied in.
- [`docker-compose.yml`](../docker-compose.yml): Orchestrates `lb`, four `backend` instances, the `health_checker`, and `redis`.
