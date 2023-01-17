# Use rust image as the builder base
FROM rust:1.66.0-bullseye AS builder

# Set the build profile to be release
ARG BUILD_PROFILE=release
ENV BUILD_PROFILE $BUILD_PROFILE

# Update and install dependencies
RUN apt-get update && apt-get -y upgrade && apt-get install -y libclang-dev pkg-config

# Copy base folder into docker context
COPY . reth

# Build reth
RUN cd reth && cargo build --all --locked --profile $BUILD_PROFILE

# Use alpine as the release image
FROM frolvlad/alpine-glibc

RUN apk add --no-cache linux-headers

# Copy the built reth binary from the previous stage
COPY --from=builder /reth/target/release/reth /usr/local/bin/reth

CMD ["/usr/local/bin/reth"]
