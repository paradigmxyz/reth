#!/usr/bin/env bash
set -euo pipefail

BIG_BLOCKS_DIR="$HOME/.reth-bench-big-blocks"
PAYLOAD_DIR="$BIG_BLOCKS_DIR/payloads"
MANIFEST="$BIG_BLOCKS_DIR/manifest.json"
echo "BENCH_BIG_BLOCKS_DIR=${BIG_BLOCKS_DIR}" >> "$GITHUB_ENV"

if [ ! -f "$MANIFEST" ]; then
  echo "::error::Missing local big-blocks manifest at $MANIFEST"
  exit 1
fi

BASE_SNAPSHOT=$(jq -r '.base_snapshot // empty' "$MANIFEST")
if [ -z "$BASE_SNAPSHOT" ]; then
  echo "::error::Big-blocks manifest missing base_snapshot field"
  exit 1
fi

if [ ! -d "$PAYLOAD_DIR" ]; then
  echo "::error::Missing local big-block payload directory at $PAYLOAD_DIR"
  exit 1
fi

PAYLOAD_COUNT=$(find "$PAYLOAD_DIR" -name '*.json' | wc -l)
if [ "$PAYLOAD_COUNT" -eq 0 ]; then
  echo "::error::No payload files found in $PAYLOAD_DIR"
  exit 1
fi

echo "Big-blocks base snapshot: $BASE_SNAPSHOT"
echo "Payload files: $PAYLOAD_COUNT"
echo "BENCH_SNAPSHOT_NAME=${BASE_SNAPSHOT}" >> "$GITHUB_ENV"
