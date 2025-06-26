#!/bin/bash

# Script to inject cargo docs into Vocs dist folder

# Define paths
CARGO_DOCS_PATH="../../target/doc"
VOCS_DIST_PATH="./docs/dist/docs"

echo "Injecting cargo docs into Vocs dist..."

# Check if cargo docs exist
if [ ! -d "$CARGO_DOCS_PATH" ]; then
    echo "Error: Cargo docs not found at $CARGO_DOCS_PATH"
    echo "Please run: cargo doc --no-deps --workspace --exclude 'example-*'"
    exit 1
fi

# Check if Vocs dist exists
if [ ! -d "./docs/dist" ]; then
    echo "Error: Vocs dist not found. Please run: bun run build"
    exit 1
fi

# Create docs directory in dist if it doesn't exist
mkdir -p "$VOCS_DIST_PATH"

# Copy all cargo docs to the dist/docs folder
echo "Copying cargo docs to $VOCS_DIST_PATH..."
cp -r "$CARGO_DOCS_PATH"/* "$VOCS_DIST_PATH/"

# Fix relative paths in HTML files to work from /reth/docs
echo "Fixing relative paths in HTML files..."
find "$VOCS_DIST_PATH" -name "*.html" -type f | while read -r file; do
    # Fix paths to static.files directory
    sed -i '' 's|href="./static\.files/|href="/reth/docs/static.files/|g' "$file"
    sed -i '' 's|src="./static\.files/|src="/reth/docs/static.files/|g' "$file"
    sed -i '' 's|href="../static\.files/|href="/reth/docs/static.files/|g' "$file"
    sed -i '' 's|src="../static\.files/|src="/reth/docs/static.files/|g' "$file"
    
    # Fix paths to crates (for navigation)
    sed -i '' 's|href="./\([^/]*\)/index\.html"|href="/reth/docs/\1/index.html"|g' "$file"
    sed -i '' 's|href="../\([^/]*\)/index\.html"|href="/reth/docs/\1/index.html"|g' "$file"
    
    # Fix root path references
    sed -i '' 's|href="./index\.html"|href="/reth/docs/index.html"|g' "$file"
    sed -i '' 's|href="../index\.html"|href="/reth/docs/index.html"|g' "$file"
    
    # Fix data-root-path and data-static-root-path attributes
    sed -i '' 's|data-root-path="./"|data-root-path="/reth/docs/"|g' "$file"
    sed -i '' 's|data-root-path="../"|data-root-path="/reth/docs/"|g' "$file"
    sed -i '' 's|data-static-root-path="./static\.files/"|data-static-root-path="/reth/docs/static.files/"|g' "$file"
    sed -i '' 's|data-static-root-path="../static\.files/"|data-static-root-path="/reth/docs/static.files/"|g' "$file"
done

echo "Cargo docs successfully injected!"
echo "The crate documentation will be available at /reth/docs"