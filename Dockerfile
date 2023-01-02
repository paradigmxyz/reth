# Use latest rust image as the builder base
FROM rust:1.66.0-bullseye AS builder

# Set the build profile to be release
ARG BUILD_PROFILE=release
ENV BUILD_PROFILE $BUILD_PROFILE

# Update and install dependencies
RUN apt-get update && apt-get -y upgrade && apt-get install -y libclang-dev pkg-config

# Copy base folder into docker context
COPY . reth

# Build reth
RUN cd reth && cargo build --all --profile release

# Use ubuntu as the release image
FROM ubuntu:22.04

# Update dependencies and add ethereum ppa
RUN apt-get update && apt-get -y upgrade && apt-get install -y --no-install-recommends \
  libssl-dev \
  ca-certificates \
  gnupg2 \
  software-properties-common && apt-key update \
  && add-apt-repository -y ppa:ethereum/ethereum

# Install Geth as it's a dependency for reth
RUN apt-get update && apt-get install -y ethereum \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

# Copy the built reth binary from the previous stage
COPY --from=builder /reth/target/release/reth /usr/local/bin/reth

CMD ["/usr/local/bin/reth"]
