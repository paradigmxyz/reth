#!/usr/bin/env bash
# PGO+BOLT optimized build script for reth/op-reth.
#
# Environment variables:
#   BINARY   - Binary to build: reth (default) or op-reth
#   PROFILE  - Cargo profile (default: maxperf)
#   FEATURES - Cargo features (default: jemalloc,asm-keccak,min-debug-logs)
#   TARGET   - Target triple (default: auto-detected from rustc)
set -euo pipefail

cd "$(dirname "$0")/../.."

# Configuration from environment
BINARY="${BINARY:-reth}"
PROFILE="${PROFILE:-maxperf}"
FEATURES="${FEATURES:-jemalloc,asm-keccak,min-debug-logs}"
TARGET="${TARGET:-$(rustc -Vv | grep host | cut -d' ' -f2)}"

# Manifest path for the binary
case "$BINARY" in
    reth)
        MANIFEST_PATH="bin/reth"
        ;;
    op-reth)
        MANIFEST_PATH="crates/optimism/bin"
        ;;
    *)
        echo "Unknown binary: $BINARY. Supported: reth, op-reth"
        exit 1
        ;;
esac

LLVM_VERSION=$(rustc -Vv | grep -oP 'LLVM version: \K\d+')

# Enable debug symbols for BOLT (requires symbols to reorder code)
# Convert profile name to uppercase for Cargo env var
PROFILE_UPPER=$(echo "$PROFILE" | tr '[:lower:]' '[:upper:]')
export "CARGO_PROFILE_${PROFILE_UPPER}_STRIP=debuginfo"

echo "=== PGO+BOLT Build ==="
echo "Binary: $BINARY"
echo "Manifest: $MANIFEST_PATH"
echo "Target: $TARGET"
echo "Profile: $PROFILE"
echo "Features: $FEATURES"
echo "LLVM Version: $LLVM_VERSION"

CARGO_ARGS=(--profile "$PROFILE" --features "$FEATURES" --manifest-path "$MANIFEST_PATH/Cargo.toml" --bin "$BINARY" --locked)

install_bolt() {
    if command -v llvm-bolt &>/dev/null; then
        echo "BOLT already installed"
        return
    fi
    echo "Installing BOLT from apt.llvm.org..."
    wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | sudo tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc >/dev/null
    CODENAME=$(lsb_release -cs)
    echo "deb http://apt.llvm.org/$CODENAME/ llvm-toolchain-$CODENAME-$LLVM_VERSION main" | sudo tee /etc/apt/sources.list.d/llvm.list >/dev/null
    sudo apt-get update -qq
    sudo apt-get install -y -qq "bolt-$LLVM_VERSION"
    sudo ln -sf "/usr/bin/llvm-bolt-$LLVM_VERSION" /usr/local/bin/llvm-bolt
    sudo ln -sf "/usr/bin/merge-fdata-$LLVM_VERSION" /usr/local/bin/merge-fdata
}

run() {
    "$1" --help &>/dev/null || true
}

export LLVM_PROFILE_FILE=$PWD/target/pgo-profiles/${BINARY}_%m_%p.profraw

echo "Installing cargo-pgo..."
cargo install cargo-pgo --quiet
rustup component add llvm-tools-preview
install_bolt
cargo pgo info

# PGO: build instrumented, run, gather profiles
echo "=== PGO Phase ==="
echo "Building PGO-instrumented binary..."
cargo pgo build -- "${CARGO_ARGS[@]}"
echo "Running instrumented binary to gather profiles..."
run "target/$TARGET/$PROFILE/$BINARY"

# BOLT: build instrumented with PGO, run, optimize
echo "=== BOLT Phase ==="
echo "Building BOLT-instrumented binary with PGO..."
cargo pgo bolt build --with-pgo -- "${CARGO_ARGS[@]}"
echo "Running BOLT-instrumented binary..."
run "target/$TARGET/$PROFILE/$BINARY-bolt-instrumented"
echo "Optimizing with BOLT..."
cargo pgo bolt optimize --with-pgo -- "${CARGO_ARGS[@]}"

# Strip and copy optimized binary to expected locations
OPTIMIZED_BIN="target/$TARGET/$PROFILE/$BINARY-bolt-optimized"
echo "Stripping debug symbols..."
strip "$OPTIMIZED_BIN"

for out in "target/$TARGET/$PROFILE" "target/$PROFILE"; do
    mkdir -p "$out"
    cp "$OPTIMIZED_BIN" "$out/$BINARY"
done

echo "=== Build Complete ==="
ls -lh "target/$PROFILE/$BINARY"
