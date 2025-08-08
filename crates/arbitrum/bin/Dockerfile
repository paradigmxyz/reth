# syntax=docker/dockerfile:1

FROM rust:1.78 as builder
WORKDIR /build
COPY . /build
RUN cargo build --release -p reth-arbitrum-bin

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/arb-reth /usr/local/bin/arb-reth
ENTRYPOINT ["/usr/local/bin/arb-reth"]
