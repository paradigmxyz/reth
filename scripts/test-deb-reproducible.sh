#!/bin/bash
set -e

echo "Testing cargo-deb package reproducibility..."

# Clean up any existing packages
echo "Cleaning up existing packages..."
rm -f reth_*.deb reth-deb-*.deb *-diff.txt

# Build first package
echo "Building first package..."
make clean || true
make deb-cargo
FIRST_PACKAGE=$(find target/x86_64-unknown-linux-gnu/debian -name "*.deb" | head -1)
if [ -n "$FIRST_PACKAGE" ]; then
    cp "$FIRST_PACKAGE" ./reth-deb-build-1.deb
else
    echo "❌ First package not found"
    exit 1
fi

# Build second package
echo "Building second package..."
make clean || true
make deb-cargo
SECOND_PACKAGE=$(find target/x86_64-unknown-linux-gnu/debian -name "*.deb" | head -1)
if [ -n "$SECOND_PACKAGE" ]; then
    cp "$SECOND_PACKAGE" ./reth-deb-build-2.deb
else
    echo "❌ Second package not found"
    exit 1
fi

# Compare packages
echo "Comparing packages..."
echo "=== Package sizes ==="
ls -la reth-deb-build-*.deb
echo "=== SHA256 checksums ==="
sha256sum reth-deb-build-*.deb

if cmp -s reth-deb-build-1.deb reth-deb-build-2.deb; then
    echo "✅ SUCCESS: cargo-deb packages are identical!"
    echo "✅ Reproducible build PASSED"
else
    echo "❌ FAILED: cargo-deb packages differ"
    echo "Running detailed analysis with diffoscope..."
    diffoscope --text reth-deb-build-1.deb reth-deb-build-2.deb > cargo-deb-diff.txt || true
    echo "Differences saved to cargo-deb-diff.txt"
    echo "❌ Reproducible build FAILED"
    exit 1
fi
