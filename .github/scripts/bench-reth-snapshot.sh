#!/usr/bin/env bash
#
# Downloads the latest snapshot into the schelk volume using
# `reth download` with progress reporting to the GitHub PR comment.
#
# Skips the download if the manifest content hasn't changed since
# the last successful download (checked via SHA-256 of the manifest).
#
# Usage: bench-reth-snapshot.sh [--check]
#   --check   Only check if a download is needed; exits 0 if up-to-date, 1 if not.
#
# Required env:
#   SCHELK_MOUNT       – schelk mount point (e.g. /reth-bench)
#   BENCH_RETH_BINARY  – path to the reth binary (required for download, not --check)
#   GITHUB_TOKEN       – token for GitHub API calls (only for download)
#   BENCH_COMMENT_ID   – PR comment ID to update (optional)
#   BENCH_REPO         – owner/repo (e.g. paradigmxyz/reth)
#   BENCH_JOB_URL      – link to the Actions job
#   BENCH_ACTOR        – user who triggered the benchmark
#   BENCH_CONFIG       – config summary line
set -euo pipefail

MC="mc"
BUCKET="minio/reth-snapshots"
MANIFEST_PATH="reth-1-minimal-stable/manifest.json"
DATADIR="$SCHELK_MOUNT/datadir"
HASH_FILE="$HOME/.reth-bench-snapshot-hash"

# Fetch manifest and compute content hash for reliable freshness check
if ! REMOTE_HASH=$($MC cat "${BUCKET}/${MANIFEST_PATH}" 2>/dev/null | sha256sum | awk '{print $1}'); then
  echo "::error::Failed to fetch snapshot manifest from ${BUCKET}/${MANIFEST_PATH}"
  exit 2
fi

LOCAL_HASH=""
[ -f "$HASH_FILE" ] && LOCAL_HASH=$(cat "$HASH_FILE")

if [ "$REMOTE_HASH" = "$LOCAL_HASH" ]; then
  echo "Snapshot is up-to-date (manifest hash: ${REMOTE_HASH:0:16}…)"
  exit 0
fi

echo "Snapshot needs update (local: ${LOCAL_HASH:+${LOCAL_HASH:0:16}…}${LOCAL_HASH:-<none>}, remote: ${REMOTE_HASH:0:16}…)"
if [ "${1:-}" = "--check" ]; then
  exit 10
fi

: "${BENCH_RETH_BINARY:?BENCH_RETH_BINARY must be set to the reth binary path}"
RETH="$BENCH_RETH_BINARY"
if [ ! -x "$RETH" ]; then
  echo "::error::reth binary not found or not executable at $RETH"
  exit 1
fi

# Resolve the MinIO HTTP endpoint from the mc alias so reth can
# fetch archives over HTTP (the manifest's embedded base_url points
# to the cluster-internal address which is unreachable from runners).
MINIO_ENDPOINT=$($MC alias list minio --json | jq -r '.URL')
if [ -z "$MINIO_ENDPOINT" ] || [ "$MINIO_ENDPOINT" = "null" ]; then
  echo "::error::Failed to resolve MinIO endpoint from mc alias"
  exit 1
fi
BASE_URL="${MINIO_ENDPOINT}/reth-snapshots/reth-1-minimal-stable"

# Download manifest and replace base_url with the runner-reachable endpoint
MANIFEST_TMP=$(mktemp --suffix=.json)
trap 'rm -f -- "$MANIFEST_TMP"' EXIT
$MC cat "${BUCKET}/${MANIFEST_PATH}" \
  | jq --arg base "$BASE_URL" '.base_url = $base' > "$MANIFEST_TMP"

# Prepare mount
mountpoint -q "$SCHELK_MOUNT" && sudo schelk recover -y || true
sudo schelk mount -y
sudo rm -rf "$DATADIR"
sudo mkdir -p "$DATADIR"
sudo chown -R "$(id -u):$(id -g)" "$DATADIR"

update_comment() {
  local status="$1"
  [ -z "${BENCH_COMMENT_ID:-}" ] && return 0
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

update_comment "Downloading snapshot…"

# Download using reth download (manifest-path with rewritten base_url)
"$RETH" download \
  --manifest-path "$MANIFEST_TMP" \
  -y \
  --minimal \
  --datadir "$DATADIR"

update_comment "Downloading snapshot… done"
echo "Snapshot download complete"

# Sanity check: verify expected directories exist
if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
  echo "::error::Snapshot download did not produce expected directory layout (missing db/ or static_files/)"
  ls -la "$DATADIR" || true
  exit 1
fi

# Promote the new snapshot to become the schelk baseline (virgin volume).
# This copies changed blocks from scratch → virgin so that future
# `schelk recover` calls restore to this new state.
sync
sudo schelk promote -y

# Save manifest hash
echo "$REMOTE_HASH" > "$HASH_FILE"
echo "Snapshot promoted to schelk baseline (manifest hash: ${REMOTE_HASH:0:16}…)"
