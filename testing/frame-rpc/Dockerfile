FROM rust:1.97.1-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends clang libclang-dev cmake pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
ENV CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git --mount=type=cache,target=/src/target \
    cargo build --locked --bin reth --no-default-features --features reth-revm/portable && cp target/debug/reth /reth

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /reth /usr/local/bin/reth
ENTRYPOINT ["reth"]
