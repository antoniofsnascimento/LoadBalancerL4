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

## Section 2: Project Structure

To maintain scalability, the codebase avoids a monolithic `main.rs` file. The logic is decoupled into a clear library structure:

- [`src/main.rs`](../src/main.rs): Serves as the Entry point, initializing the backend pool and the accept loop.
- [`src/lib.rs`](../src/lib.rs): Handles Module declarations mapping.
- [`src/models.rs`](../src/models.rs): Contains Data structures like the `Backend` node representation.
- [`src/proxy.rs`](../src/proxy.rs): Contains the Core networking logic, including TCP streaming and bidirectional copy.
