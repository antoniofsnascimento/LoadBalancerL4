FROM rust:1.94-slim AS builder
WORKDIR /usr/src/app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /usr/src/app/target/release/load_balancer_l4 /usr/local/bin/
COPY --from=builder /usr/src/app/target/release/backend /usr/local/bin/
COPY --from=builder /usr/src/app/target/release/test_client /usr/local/bin/
COPY --from=builder /usr/src/app/target/release/health_checker /usr/local/bin/
COPY --from=builder /usr/src/app/target/release/load_tester /usr/local/bin/

ENV PATH="/usr/local/bin:${PATH}"
