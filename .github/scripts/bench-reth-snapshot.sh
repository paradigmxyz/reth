#!/usr/bin/env bash
#
# Validates that the benchmark snapshot has already been populated into the
# local schelk volume.
#
# Usage: bench-reth-snapshot.sh [--check]
#   --check   Exit 0 if the local snapshot is ready, 10 if it is missing.
#
# Required env:
#   SCHELK_MOUNT – schelk mount point (e.g. /reth-bench)
# Optional env:
#   BENCH_BIG_BLOCKS   – true when validating the big-blocks snapshot datadir
#   BENCH_SNAPSHOT_NAME – expected snapshot label for log/error output
set -euxo pipefail

: "${SCHELK_MOUNT:?SCHELK_MOUNT must be set}"

DATADIR_NAME="datadir"
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"

describe_snapshot() {
  if [ -n "${BENCH_SNAPSHOT_NAME:-}" ]; then
    printf '%s' "${BENCH_SNAPSHOT_NAME}"
  elif [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
    printf '%s' 'big-block weekly snapshot'
  else
    printf '%s' 'benchmark snapshot'
  fi
}

snapshot_ready() {
  [ -d "$DATADIR/db" ] && [ -d "$DATADIR/static_files" ]
}

EXPECTED_SNAPSHOT="$(describe_snapshot)"

sudo schelk recover -y --kill || sudo schelk full-recover -y || true
sudo schelk mount -y || true

if snapshot_ready; then
  echo "Found local ${EXPECTED_SNAPSHOT} at ${DATADIR}"
  exit 0
fi

echo "::error::Missing local ${EXPECTED_SNAPSHOT} at ${DATADIR}. Benchmarks no longer download snapshots; pre-populate the local schelk data first."
ls -la "$SCHELK_MOUNT" || true
ls -la "$DATADIR" || true

if [ "${1:-}" = "--check" ]; then
  exit 10
fi

exit 1
