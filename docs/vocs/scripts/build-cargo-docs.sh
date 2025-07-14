#!/bin/bash
set -e  # Exit on error

# Script to build cargo docs with the same flags as used in CI

# Navigate to the reth root directory (two levels up from docs/vocs)
cd ../.. || { echo "Failed to navigate to project root"; exit 1; }

echo "Current directory: $(pwd)"
echo "Checking for Cargo.toml..."
if [ ! -f "Cargo.toml" ]; then
    echo "Error: Cargo.toml not found in $(pwd)"
    echo "This script must be run from docs/vocs directory"
    exit 1
fi

echo "Building cargo docs..."

# Build the documentation
export RUSTDOCFLAGS="--cfg docsrs --show-type-layout --generate-link-to-definition --enable-index-page -Zunstable-options"

# Run cargo doc and capture the exit code
# Use --quiet to reduce output and --timings to see what's taking time
echo "Running cargo doc (this may take a few minutes)..."
if CARGO_INCREMENTAL=0 cargo doc --no-deps --exclude "example-*" --quiet; then
    echo "Cargo doc command completed successfully"
else
    echo "Error: Cargo doc command failed with exit code $?"
    echo "Trying with less features..."
    # Try building just the core crates if full build fails
    if cargo doc -p reth -p reth-node -p reth-primitives --no-deps --quiet; then
        echo "Built docs for core crates"
    else
        echo "Error: Even core crates failed to build docs"
        exit 1
    fi
fi

# Verify that docs were actually created
if [ -d "./target/doc" ]; then
    file_count=$(find ./target/doc -name "*.html" | wc -l)
    echo "Cargo docs built successfully at ./target/doc"
    echo "Found $file_count HTML files"
    
    # List some of the generated crates for debugging
    echo "Generated documentation for crates:"
    ls -la ./target/doc/ | head -20
else
    echo "Error: ./target/doc directory was not created"
    exit 1
fi