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

# txgen is private. Use the git CLI so cargo honors the workflow's
# url.*.insteadOf auth configuration from the dependency install step.
export CARGO_NET_GIT_FETCH_WITH_CLI=true

cargo install --git "$TXGEN_REPO" --rev "$TXGEN_REV" txgen-ethereum --bin txgen-ethereum --locked
cargo install --git "$TXGEN_REPO" --rev "$TXGEN_REV" bench-cli --bin bench --locked
