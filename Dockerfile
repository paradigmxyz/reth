FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR app

# Builds a cargo-chef plan
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Set the build profile to be release
ARG BUILD_PROFILE=maxperf
ENV BUILD_PROFILE $BUILD_PROFILE

# Install system dependencies
RUN apt-get update && apt-get -y upgrade && apt-get install -y libclang-dev pkg-config

# Builds dependencies
RUN cargo chef cook --profile $BUILD_PROFILE --recipe-path recipe.json

# Build application
COPY . .
RUN cargo build --profile $BUILD_PROFILE --locked --bin reth

# Use Ubuntu as the release image
FROM ubuntu AS runtime
WORKDIR app

# Copy reth over from the build stage
COPY --from=builder /app/target/release/reth /usr/local/bin

EXPOSE 30303 30303/udp 9000 8545 8546
ENTRYPOINT ["/usr/local/bin/reth", "node"]
