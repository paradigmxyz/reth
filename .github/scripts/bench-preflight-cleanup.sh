#!/usr/bin/env bash
set -euo pipefail

sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
sudo schelk recover -y --kill || sudo schelk full-recover -y || true
rm -rf "$BENCH_WORK_DIR"
mkdir -p "$BENCH_WORK_DIR"
