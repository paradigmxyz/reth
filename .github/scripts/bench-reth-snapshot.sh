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

# Get remote ETag
REMOTE_ETAG=$(mc stat "$BUCKET" 2>/dev/null | grep -i '^ETag' | awk '{print $3}')
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

# Query total snapshot size for progress tracking
TOTAL_BYTES=$(mc stat "$BUCKET" 2>/dev/null | grep -i '^Size' | awk '{print $3}')
if [ -z "$TOTAL_BYTES" ]; then
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
  local status="Building binaries \\& downloading snapshot… ${pct}%"
  local body="cc @${BENCH_ACTOR}\n\n🚀 Benchmark started! [View job](${BENCH_JOB_URL})\n\n⏳ **Status:** ${status}\n\n${BENCH_CONFIG}"
  curl -sf -X PATCH \
    -H "Authorization: token ${GITHUB_TOKEN}" \
    -H "Accept: application/vnd.github.v3+json" \
    "https://api.github.com/repos/${BENCH_REPO}/issues/comments/${BENCH_COMMENT_ID}" \
    -d "$(jq -nc --arg body "$body" '{body: $body}')" \
    > /dev/null 2>&1 || true
}

# Start progress reporter in background
(
  while true; do
    sleep 10
    CURRENT=$(du -sb "$DATADIR" 2>/dev/null | awk '{print $1}')
    CURRENT=${CURRENT:-0}
    if [ "$TOTAL_BYTES" -gt 0 ]; then
      PCT=$(( CURRENT * 100 / TOTAL_BYTES ))
      [ "$PCT" -gt 100 ] && PCT=100
      echo "Snapshot download: $(numfmt --to=iec "$CURRENT") / $(numfmt --to=iec "$TOTAL_BYTES") (${PCT}%)"
      update_comment "$PCT"
    fi
  done
) &
PROGRESS_PID=$!
trap 'kill $PROGRESS_PID 2>/dev/null || true' EXIT

# Download and extract
mc cat "$BUCKET" | pzstd -d -p 6 | sudo tar -xf - -C "$DATADIR"

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
