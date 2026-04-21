#!/usr/bin/env bash
#
# Downloads the latest snapshot into the schelk volume using
# `reth download` with progress reporting to the GitHub PR comment.
#
# Skips the download if the manifest content hasn't changed since
# the last successful download (checked via SHA-256 of the manifest).
#
# Usage: bench-reth-snapshot.sh [--check]
#   --check   Only check if a download is needed; exits 0 if up-to-date, 10 if not.
#
# Required env:
#   SCHELK_MOUNT       – schelk mount point (e.g. /reth-bench)
#   BENCH_RETH_BINARY  – path to the reth binary
#   GITHUB_TOKEN       – token for GitHub API calls (only for download)
#   BENCH_COMMENT_ID   – PR comment ID to update (optional)
#   BENCH_REPO         – owner/repo (e.g. paradigmxyz/reth)
#   BENCH_JOB_URL      – link to the Actions job
#   BENCH_ACTOR        – user who triggered the benchmark
#   BENCH_CONFIG       – config summary line
set -euxo pipefail

MC="mc"
DATADIR_NAME="datadir"
HASH_MODE_SUFFIX=""
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
  HASH_MODE_SUFFIX="-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"
HASH_FILE="$HOME/.reth-bench-snapshot-hash${HASH_MODE_SUFFIX}"

resolve_bucket() {
  local candidate

  for candidate in \
    "${BENCH_SNAPSHOT_BUCKET:-}" \
    "r2-reth-snapshots/reth-snapshots" \
    "minio/reth-snapshots"; do
    if [ -n "$candidate" ] && $MC ls "$candidate" >/dev/null 2>&1; then
      echo "$candidate"
      return 0
    fi
  done

  echo "::error::Failed to find a readable snapshot bucket via mc ls"
  exit 2
}

resolve_snapshot_name() {
  local bucket="$1"

  # Big-block benchmarks pin the base snapshot via BENCH_SNAPSHOT_NAME.
  if [ -n "${BENCH_SNAPSHOT_NAME:-}" ]; then
    echo "$BENCH_SNAPSHOT_NAME"
    return 0
  fi

  mapfile -t snapshots < <(
    $MC ls "$bucket" 2>/dev/null \
      | awk '$NF ~ /\/$/ { print $NF }' \
      | sed 's:/$::'
  )

  if [ "${#snapshots[@]}" -lt 2 ]; then
    echo "::error::Need at least two snapshots in ${bucket} to resolve the previous snapshot"
    exit 2
  fi

  echo "${snapshots[$(( ${#snapshots[@]} - 2 ))]}"
}

BUCKET="$(resolve_bucket)"
SNAPSHOT_NAME="$(resolve_snapshot_name "$BUCKET")"
MANIFEST_PATH="${SNAPSHOT_NAME}/manifest.json"
BUCKET_ALIAS="${BUCKET%%/*}"
BUCKET_PATH="${BUCKET#*/}"

echo "Using snapshot bucket: ${BUCKET}"
echo "Using snapshot: ${SNAPSHOT_NAME}"

# Fetch manifest and compute content hash for reliable freshness check
XTRACE_WAS_ENABLED=false
if [[ $- == *x* ]]; then
  XTRACE_WAS_ENABLED=true
  set +x
fi

if ! MANIFEST_CONTENT=$($MC cat "${BUCKET}/${MANIFEST_PATH}" 2>/dev/null); then
  echo "::error::Failed to fetch snapshot manifest from ${BUCKET}/${MANIFEST_PATH}"
  exit 2
fi
ORIGINAL_BASE_URL=$(printf '%s' "$MANIFEST_CONTENT" | jq -r '.base_url // empty')
REMOTE_HASH=$(printf '%s' "$MANIFEST_CONTENT" | sha256sum | awk '{print $1}')

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

RETH="${BENCH_RETH_BINARY:?BENCH_RETH_BINARY must be set}"
if [ ! -x "$RETH" ]; then
  echo "::error::reth binary not found or not executable at $RETH"
  exit 1
fi

# Resolve the MinIO HTTP endpoint from the mc alias so reth can
# fetch archives over HTTP (the manifest's embedded base_url points
# to the cluster-internal address which is unreachable from runners).
MINIO_ENDPOINT=$($MC alias list "$BUCKET_ALIAS" --json 2>/dev/null | jq -r '.URL // empty') || true
if [ -z "$MINIO_ENDPOINT" ]; then
  echo "::error::Failed to resolve snapshot endpoint from mc alias '${BUCKET_ALIAS}'"
  exit 1
fi

MANIFEST_TMP=$(mktemp --suffix=.json)
trap 'rm -f -- "$MANIFEST_TMP"' EXIT
if [[ "$MINIO_ENDPOINT" == *".r2.cloudflarestorage.com" ]]; then
  # R2's S3 endpoint is not the public object-download URL. Keep the manifest's
  # published base_url (for example r2.dev or a custom domain) for anonymous GETs.
  printf '%s' "$MANIFEST_CONTENT" > "$MANIFEST_TMP"
else
  BASE_URL="${MINIO_ENDPOINT%/}/${BUCKET_PATH}/${SNAPSHOT_NAME}"
  # Rewrite manifest's base_url with the runner-reachable endpoint.
  printf '%s' "$MANIFEST_CONTENT" \
    | jq --arg base "$BASE_URL" '.base_url = $base' > "$MANIFEST_TMP"
fi
EFFECTIVE_BASE_URL=$(jq -r '.base_url // empty' "$MANIFEST_TMP")

if [ "$XTRACE_WAS_ENABLED" = true ]; then
  set -x
fi

echo "Manifest base_url before rewrite: ${ORIGINAL_BASE_URL:-<empty>}"
if [[ "$MINIO_ENDPOINT" == *".r2.cloudflarestorage.com" ]]; then
  echo "Using manifest base_url for R2 snapshot downloads"
fi
echo "Manifest base_url after rewrite: ${EFFECTIVE_BASE_URL:-<empty>}"

# Prepare mount. If a previous run left the volume mounted, recover first.
sudo schelk recover -y --kill || true
sudo schelk mount -y
sudo rm -rf "$DATADIR"
sudo mkdir -p "$DATADIR"
# reth download runs as current user (not root), needs write access
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
