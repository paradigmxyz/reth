# syntax=docker.io/docker/dockerfile:1.7-labs

FROM lukemathwalker/cargo-chef:latest-rust-1.93 AS chef
WORKDIR /app

LABEL org.opencontainers.image.source=https://github.com/paradigmxyz/reth
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"

# Install system dependencies
RUN apt-get update && apt-get install -y libclang-dev pkg-config

# Builds a cargo-chef plan
FROM chef AS planner
COPY --exclude=.git --exclude=dist . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Build profile, release by default
ARG BUILD_PROFILE=maxperf
ENV BUILD_PROFILE=$BUILD_PROFILE

# Extra Cargo flags
ARG RUSTFLAGS=""
ENV RUSTFLAGS="$RUSTFLAGS"

# Extra Cargo features
ARG FEATURES=""
ENV FEATURES=$FEATURES

# Builds dependencies
RUN cargo chef cook --profile $BUILD_PROFILE --features "$FEATURES" --recipe-path recipe.json

# Build application
# Platform-specific RUSTFLAGS: amd64 uses x86-64-v3 (Haswell+) with pclmulqdq for rocksdb
#
# TARGETPLATFORM is set by BuildKit: https://docs.docker.com/reference/dockerfile#automatic-platform-args-in-the-global-scope
ARG TARGETPLATFORM
COPY --exclude=dist . .
RUN if [ -n "$RUSTFLAGS" ]; then \
        export RUSTFLAGS="$RUSTFLAGS"; \
    elif [ "$TARGETPLATFORM" = "linux/amd64" ]; then \
        export RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq"; \
    fi && \
    cargo build --profile $BUILD_PROFILE --features "$FEATURES" --locked --bin reth

# ARG is not resolved in COPY so we have to hack around it by copying the
# binary to a temporary location
RUN cp /app/target/$BUILD_PROFILE/reth /app/reth

# Use Ubuntu as the release image
FROM ubuntu:24.04 AS runtime
WORKDIR /app

# Copy reth over from the build stage
COPY --from=builder /app/reth /usr/local/bin

# Copy licenses
COPY LICENSE-* ./

EXPOSE 30303 30303/udp 9001 8545 8546
ENTRYPOINT ["/usr/local/bin/reth"]
