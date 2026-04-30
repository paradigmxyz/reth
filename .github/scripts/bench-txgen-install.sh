#!/usr/bin/env bash
#
# Installs the txgen tools used by the txgen-backed PR benchmark path.
# Keep this separate from bench-reth-build.sh so scheduled benchmarks can keep
# using the legacy reth-bench runner until they are migrated explicitly.
#
# Required env:
#   TXGEN_REV   – pinned txgen git revision
# Optional env:
#   TXGEN_REPO  – txgen repository URL (default: https://github.com/tempoxyz/txgen)
set -euxo pipefail

: "${TXGEN_REV:?TXGEN_REV must be set to a pinned txgen revision}"

TXGEN_REPO="${TXGEN_REPO:-https://github.com/tempoxyz/txgen}"

cargo install --git "$TXGEN_REPO" --rev "$TXGEN_REV" -p txgen-ethereum --bin txgen-ethereum --locked
cargo install --git "$TXGEN_REPO" --rev "$TXGEN_REV" -p bench-cli --bin bench --locked
