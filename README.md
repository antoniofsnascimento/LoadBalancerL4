# L4 TCP Load Balancer in Rust

A high-performance Layer 4 (Transport) TCP Load Balancer implemented in Rust. This project focuses on memory safety, asynchronous concurrency, and low-latency traffic distribution without relying on a garbage collector.

## Tech Stack

- **Language:** [Rust (Edition 2024)](https://www.rust-lang.org/)
- **Runtime:** [Tokio v1](https://tokio.rs/) (Asynchronous I/O)
- **Layer:** OSI Layer 4 (TCP)

## Project Documentation Map

To understand the engineering principles behind this project, follow this reading guide:

1. [**Architecture Design**](docs/architecture.md): Justification for the system structure, concurrency model, and folder organization.
2. [**TCP Proxy Implementation**](docs/tcpProxy.md): A technical, line-by-line deep dive into the network engine and data routing.

## Getting Started

### Prerequisites

Ensure you have the [Rust toolchain](https://www.rust-lang.org/tools/install) installed.

### Installation

1. Clone the repository:
   ```bash
   git clone <repository-url>
   cd LoadBalancerL4
   ```

2. Compile the project:
   ```bash
   cargo build
   ```

3. Run the project in development mode:
   ```bash
   cargo run
   ```
