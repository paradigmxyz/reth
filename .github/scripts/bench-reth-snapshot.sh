#!/usr/bin/env bash
#
# Ensures the benchmark snapshot in the schelk volume matches the configured
# manifest. If the local manifest marker differs from the remote manifest, the
# snapshot is replaced via `reth download` and promoted as the new schelk
# baseline.
#
# Usage: bench-reth-snapshot.sh [--check]
#   --check   Exit 0 if the local snapshot is current, 10 if it needs refresh.
#
# Required env:
#   SCHELK_MOUNT       - schelk mount point (e.g. /reth-bench)
#   BENCH_SNAPSHOT_MANIFEST_URL - exact manifest URL to sync from
#   BENCH_RETH_BINARY  - path to the reth-compatible binary (required to refresh)
#
# Optional env:
#   BENCH_BIG_BLOCKS            - true when syncing the big-blocks datadir
set -euo pipefail

: "${SCHELK_MOUNT:?SCHELK_MOUNT must be set}"
: "${BENCH_SNAPSHOT_MANIFEST_URL:?BENCH_SNAPSHOT_MANIFEST_URL must be set}"

if [ "${1:-}" != "" ] && [ "${1:-}" != "--check" ]; then
  echo "Usage: $0 [--check]"
  exit 1
fi

CHECK_ONLY=false
if [ "${1:-}" = "--check" ]; then
  CHECK_ONLY=true
fi

DATADIR_NAME="datadir"
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"
LOCAL_MANIFEST="$DATADIR/manifest.json"

MANIFEST_URL="$BENCH_SNAPSHOT_MANIFEST_URL"
MANIFEST_BASE_URL="${MANIFEST_URL%/*}"

REMOTE_MANIFEST="$(mktemp "${TMPDIR:-/tmp}/reth-bench-remote-manifest.XXXXXX")"
REMOTE_CANONICAL="$(mktemp "${TMPDIR:-/tmp}/reth-bench-remote-canonical.XXXXXX")"
LOCAL_CANONICAL="$(mktemp "${TMPDIR:-/tmp}/reth-bench-local-canonical.XXXXXX")"
DOWNLOAD_MANIFEST="$(mktemp "${TMPDIR:-/tmp}/reth-bench-download-manifest.XXXXXX")"
trap 'rm -f "$REMOTE_MANIFEST" "$REMOTE_CANONICAL" "$LOCAL_CANONICAL" "$DOWNLOAD_MANIFEST"' EXIT

snapshot_ready() {
  [ -d "$DATADIR/db" ] && [ -d "$DATADIR/static_files" ]
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

echo "Using snapshot manifest from BENCH_SNAPSHOT_MANIFEST_URL"

if ! curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 10 "$MANIFEST_URL" -o "$REMOTE_MANIFEST"; then
  echo "::error::Failed to fetch snapshot manifest from BENCH_SNAPSHOT_MANIFEST_URL"
  exit 2
fi

if ! jq -S . "$REMOTE_MANIFEST" > "$REMOTE_CANONICAL"; then
  echo "::error::Snapshot manifest is not valid JSON"
  exit 2
fi
REMOTE_HASH="$(sha256_file "$REMOTE_CANONICAL")"

sudo schelk recover -y --kill || sudo schelk full-recover -y || true
sudo schelk mount -y

LOCAL_HASH=""
LOCAL_MATCH=false
if [ -f "$LOCAL_MANIFEST" ] && jq -S . "$LOCAL_MANIFEST" > "$LOCAL_CANONICAL"; then
  LOCAL_HASH="$(sha256_file "$LOCAL_CANONICAL")"
  if cmp -s "$REMOTE_CANONICAL" "$LOCAL_CANONICAL"; then
    LOCAL_MATCH=true
  fi
fi

if [ "$LOCAL_MATCH" = true ] && snapshot_ready; then
  echo "Snapshot is up-to-date (manifest hash: ${REMOTE_HASH:0:16})"
  exit 0
fi

if ! snapshot_ready; then
  echo "Snapshot needs refresh: missing expected db/ or static_files/ under ${DATADIR}"
elif [ ! -f "$LOCAL_MANIFEST" ]; then
  echo "Snapshot needs refresh: missing local manifest marker at ${LOCAL_MANIFEST}"
elif [ -z "$LOCAL_HASH" ]; then
  echo "Snapshot needs refresh: local manifest marker is not valid JSON"
else
  echo "Snapshot needs refresh (local: ${LOCAL_HASH:0:16}, remote: ${REMOTE_HASH:0:16})"
fi

if [ "$CHECK_ONLY" = true ]; then
  exit 10
fi

RETH="${BENCH_RETH_BINARY:?BENCH_RETH_BINARY must be set when refreshing the snapshot}"
if [ ! -x "$RETH" ]; then
  echo "::error::reth binary not found or not executable at ${RETH}"
  exit 1
fi

# Force archive URLs to resolve relative to the configured manifest endpoint.
# Some published manifests carry a base_url that is valid for publishers but
# not for benchmark runners.
jq --arg base "$MANIFEST_BASE_URL" '.base_url = $base' "$REMOTE_MANIFEST" > "$DOWNLOAD_MANIFEST"

sudo rm -rf "$DATADIR"
sudo mkdir -p "$DATADIR"
# reth download runs as the current user and needs write access.
sudo chown -R "$(id -u):$(id -g)" "$DATADIR"

"$RETH" download \
  --manifest-path "$DOWNLOAD_MANIFEST" \
  -y \
  --minimal \
  --datadir "$DATADIR"

if ! snapshot_ready; then
  echo "::error::Snapshot download did not produce expected directory layout (missing db/ or static_files/)"
  ls -la "$DATADIR" || true
  exit 1
fi

cp "$REMOTE_MANIFEST" "$LOCAL_MANIFEST"

sync
sudo schelk promote -y

echo "Snapshot promoted to schelk baseline (manifest hash: ${REMOTE_HASH:0:16})"
