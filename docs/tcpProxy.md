# TCP Proxy Implementation

For this Layer 4 Load Balancer, the technical foundation begins with a functional TCP proxy that utilizes asynchronous I/O to manage multiple concurrent connections. Below is a detailed technical analysis of the project's core modules.

## Section 1: Data Models ([`src/models.rs`](../src/models.rs))

We define a public struct named `Backend` with a public string field named `addr`.

```rust
#[derive(Debug, Clone)]
pub struct Backend {
    pub addr: String,
}
```

- **Derive Macros:** We use `Debug` and `Clone` to instruct the compiler to automatically generate code so the structure can be printed to the console for debugging and duplicated in memory.
- **Owned Strings:** Using an owned `String` instead of a string slice guarantees that the `Backend` struct has full ownership of its data. This eliminates complex lifetime management when passing backend configurations across asynchronous tasks.

## Section 2: The Core Proxy Logic ([`src/proxy.rs`](../src/proxy.rs))

The core logic is implemented in the `handle_connection` function.

- **Asynchronous TCP:** We use [`tokio::net::TcpStream`](https://docs.rs/tokio/latest/tokio/net/struct.TcpStream.html), which is the asynchronous version of a TCP connection. It allows reading and writing bytes without freezing the CPU thread.
- **Atomic Reference Counter:** We use [`std::sync::Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html). Because Rust enforces strict ownership rules, sharing the list of backends across thousands of concurrent tasks requires `Arc`. It places the data on the Heap and hands out thread-safe reference tickets.
- **Error Handling:** The [`handle_connection`](../src/proxy.rs#L5-L8) function is marked as `async`, allowing it to suspend execution while waiting for network packets. It returns a `Result` with a dynamic `Error` box, which is Rust's robust error handling.

```rust
pub async fn handle_connection(
    mut client_stream: TcpStream,
    backends: Arc<Vec<Backend>>
) -> Result<(), Box<dyn std::error::Error>> {
    // ... logic ...
}
```

- **Bidirectional Copy:** Inside, [`tokio::io::copy_bidirectional`](../src/proxy.rs#L13-L16) is the heart of the Layer 4 proxy. It concurrently moves bytes from the client to the server and vice-versa. We use an underscore prefix on the `_from_backend` variable to signal the compiler that we are intentionally ignoring it for now.

## Section 3: The Listener Engine ([`src/main.rs`](../src/main.rs))

The listener engine orchestrates the incoming connections.

- **Async Binding:** We use `TcpListener::bind` to open the network port asynchronously. If a client is slow to connect, it will not block other operations.
- **Accept Loop:** We use an explicit infinite loop optimized by the compiler. Calling [`listener.accept().await`](../src/main.rs#L19) pauses the loop until a new TCP connection arrives.
- **Efficient Cloning:** We use `Arc::clone` to cheaply increment the atomic reference counter; it does not clone the entire list of servers, only the pointer to it.
- **Lightweight Tasks:** Finally, [`tokio::spawn`](../src/main.rs#L23) creates a lightweight asynchronous task (green thread) for the specific client. The `move` keyword forces the task to take ownership of the client stream and backend reference, guaranteeing memory safety across concurrency boundaries.
