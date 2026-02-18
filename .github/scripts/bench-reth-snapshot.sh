#!/usr/bin/env bash
#
# Downloads the latest nightly snapshot into the schelk volume with
# progress reporting to the GitHub PR comment.
#
# Skips the download if the local ETag marker matches the remote one.
#
# Usage: bench-reth-snapshot.sh [--check]
#   --check   Only check if a download is needed; exits 0 if up-to-date, 1 if not.
#
# Required env:
#   SCHELK_MOUNT       – schelk mount point (e.g. /reth-bench)
#   GITHUB_TOKEN       – token for GitHub API calls (only for download)
#   BENCH_COMMENT_ID   – PR comment ID to update (optional)
#   BENCH_REPO         – owner/repo (e.g. paradigmxyz/reth)
#   BENCH_JOB_URL      – link to the Actions job
#   BENCH_ACTOR        – user who triggered the benchmark
#   BENCH_CONFIG       – config summary line
set -euo pipefail

BUCKET="minio/reth-snapshots/reth-1-minimal-nightly-previous.tar.zst"
DATADIR="$SCHELK_MOUNT/datadir"
ETAG_FILE="$HOME/.reth-bench-snapshot-etag"

# Get remote metadata via JSON for reliable parsing
MC_STAT=$(mc stat --json "$BUCKET" 2>/dev/null || true)
REMOTE_ETAG=$(echo "$MC_STAT" | jq -r '.etag // empty')
if [ -z "$REMOTE_ETAG" ]; then
  echo "::warning::Failed to get ETag from mc stat, will re-download"
  REMOTE_ETAG="unknown-$(date +%s)"
fi

LOCAL_ETAG=""
[ -f "$ETAG_FILE" ] && LOCAL_ETAG=$(cat "$ETAG_FILE")

if [ "$REMOTE_ETAG" = "$LOCAL_ETAG" ]; then
  echo "Snapshot is up-to-date (ETag: ${REMOTE_ETAG})"
  if [ "${1:-}" = "--check" ]; then
    exit 0
  fi
  exit 0
fi

echo "Snapshot needs update (local: ${LOCAL_ETAG:-<none>}, remote: ${REMOTE_ETAG})"
if [ "${1:-}" = "--check" ]; then
  exit 1
fi

# Get compressed size for progress tracking
TOTAL_BYTES=$(echo "$MC_STAT" | jq -r '.size // empty')
if [ -z "$TOTAL_BYTES" ] || [ "$TOTAL_BYTES" = "0" ]; then
  echo "::error::Failed to get snapshot size from mc stat"
  exit 1
fi
echo "Snapshot size: $TOTAL_BYTES bytes ($(numfmt --to=iec "$TOTAL_BYTES"))"

# Prepare mount
mountpoint -q "$SCHELK_MOUNT" && sudo schelk recover -y || true
sudo schelk mount -y
sudo rm -rf "$DATADIR"
sudo mkdir -p "$DATADIR"

update_comment() {
  local pct="$1"
  [ -z "${BENCH_COMMENT_ID:-}" ] && return 0
  local status="Building binaries & downloading snapshot… ${pct}%"
  local body
  body="$(printf 'cc @%s\n\n🚀 Benchmark started! [View job](%s)\n\n⏳ **Status:** %s\n\n%s' \
    "$BENCH_ACTOR" "$BENCH_JOB_URL" "$status" "$BENCH_CONFIG")"
  curl -sf -X PATCH \
    -H "Authorization: token ${GITHUB_TOKEN}" \
    -H "Accept: application/vnd.github.v3+json" \
    "https://api.github.com/repos/${BENCH_REPO}/issues/comments/${BENCH_COMMENT_ID}" \
    -d "$(jq -nc --arg body "$body" '{body: $body}')" \
    > /dev/null 2>&1 || true
}

# Track compressed bytes flowing through the pipe
DL_BYTES_FILE=$(mktemp)
echo 0 > "$DL_BYTES_FILE"

# Start progress reporter in background
(
  while true; do
    sleep 10
    CURRENT=$(cat "$DL_BYTES_FILE" 2>/dev/null || echo 0)
    if [ "$TOTAL_BYTES" -gt 0 ]; then
      PCT=$(( CURRENT * 100 / TOTAL_BYTES ))
      [ "$PCT" -gt 100 ] && PCT=100
      echo "Snapshot download: $(numfmt --to=iec "$CURRENT") / $(numfmt --to=iec "$TOTAL_BYTES") (${PCT}%)"
      update_comment "$PCT"
    fi
  done
) &
PROGRESS_PID=$!
trap 'kill $PROGRESS_PID 2>/dev/null || true; rm -f "$DL_BYTES_FILE"' EXIT

# Download and extract; python byte counter tracks compressed bytes received
mc cat "$BUCKET" | python3 -c "
import sys
count = 0
while True:
    data = sys.stdin.buffer.read(1048576)
    if not data:
        break
    count += len(data)
    sys.stdout.buffer.write(data)
    with open('$DL_BYTES_FILE', 'w') as f:
        f.write(str(count))
" | pzstd -d -p 6 | sudo tar -xf - -C "$DATADIR"

# Stop progress reporter
kill $PROGRESS_PID 2>/dev/null || true
wait $PROGRESS_PID 2>/dev/null || true

update_comment "100"
echo "Snapshot download complete"

# Sync the new snapshot as the schelk baseline
sync
sudo schelk recover -y

# Save ETag marker
echo "$REMOTE_ETAG" > "$ETAG_FILE"
echo "Snapshot synced to schelk (ETag: ${REMOTE_ETAG})"
