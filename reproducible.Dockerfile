# Use the Rust 1.80 image based on Debian Bullseye
FROM rust:1.80-bullseye@sha256:c1490599f028ae06740706279a81c09687dde26c2d00f7160b85f63e9f6d8607 as builder

# Install specific version of libclang-dev
RUN apt-get update && apt-get install -y libclang-dev=1:11.0-51+nmu5

# Set environment variables for reproducibility
ARG RUSTFLAGS="-C link-arg=-Wl,--build-id=none -Clink-arg=-static-libgcc -C metadata='' --remap-path-prefix $$(pwd)=."
ENV SOURCE_DATE_EPOCH=1724346102 \
    CARGO_INCREMENTAL=0 \
    LC_ALL=C \
    TZ=UTC \
    RUSTFLAGS="$RUSTFLAGS"

# Set the default features if not provided
ARG FEATURES="jemalloc asm-keccak"

# Clone the repository at the specific branch
RUN git clone https://github.com/MoeMahhouk/reth /app
WORKDIR /app

# Checkout the reproducible-build branch
RUN git checkout reproducible-build

# Build the project with the reproducible settings
RUN cargo build --bin reth --features "${FEATURES}" --profile "reproducible" --locked

# Create a minimal final image with just the binary
FROM scratch as binaries

# Copy the compiled binary from the builder stage
COPY --from=builder /app/target/reproducible/reth /reth

