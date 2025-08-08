# syntax=docker/dockerfile:1

FROM rust:1.81 as builder
WORKDIR /build
# Install git to fetch external workspace deps
RUN apt-get update && apt-get install -y git ca-certificates clang libclang-dev llvm-dev pkg-config build-essential && rm -rf /var/lib/apt/lists/*
# Clone arb-alloy into the expected path inside the container (reth expects /arb-alloy/*)
RUN git clone https://github.com/tiljrd/arb-alloy /arb-alloy --depth 1
COPY . /build
RUN rustup toolchain install nightly && rustup default nightly

RUN cargo build --release -p arb-reth

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/arb-reth /usr/local/bin/arb-reth
ENTRYPOINT ["/usr/local/bin/arb-reth"]
