#!/usr/bin/env bash
# Patches alloy dependencies in Cargo.toml for testing breaking changes.
#
# Usage:
#   ./scripts/patch-alloy.sh [--alloy <branch>] [--evm <branch>] [--op <branch>]
#
# Examples:
#   ./scripts/patch-alloy.sh --alloy main
#   ./scripts/patch-alloy.sh --alloy feat/new-api --evm main
#   ./scripts/patch-alloy.sh --alloy main --evm main --op main

set -euo pipefail

ALLOY_BRANCH=""
ALLOY_EVM_BRANCH=""
OP_ALLOY_BRANCH=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --alloy)
            ALLOY_BRANCH="$2"
            shift 2
            ;;
        --evm)
            ALLOY_EVM_BRANCH="$2"
            shift 2
            ;;
        --op)
            OP_ALLOY_BRANCH="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [--alloy <branch>] [--evm <branch>] [--op <branch>]"
            echo ""
            echo "Options:"
            echo "  --alloy <branch>  Patch alloy-rs/alloy crates"
            echo "  --evm <branch>    Patch alloy-rs/evm crates (alloy-evm, alloy-op-evm)"
            echo "  --op <branch>     Patch alloy-rs/op-alloy crates"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

if [[ -z "$ALLOY_BRANCH" && -z "$ALLOY_EVM_BRANCH" && -z "$OP_ALLOY_BRANCH" ]]; then
    echo "Error: At least one of --alloy, --evm, or --op must be specified"
    exit 1
fi

CARGO_TOML="${CARGO_TOML:-Cargo.toml}"

echo "Patching $CARGO_TOML..."
echo "" >> "$CARGO_TOML"

if [[ -n "$ALLOY_BRANCH" ]]; then
    echo "Patching alloy-rs/alloy with branch: $ALLOY_BRANCH"
    cat >> "$CARGO_TOML" << EOF
# Patched by patch-alloy.sh
alloy-consensus = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-contract = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-eips = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-genesis = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-json-rpc = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-network = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-network-primitives = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-provider = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-pubsub = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-client = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-admin = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-anvil = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-beacon = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-debug = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-engine = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-eth = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-mev = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-trace = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-rpc-types-txpool = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-serde = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-signer = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-signer-local = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-transport = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-transport-http = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-transport-ipc = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
alloy-transport-ws = { git = "https://github.com/alloy-rs/alloy", branch = "$ALLOY_BRANCH" }
EOF
fi

if [[ -n "$ALLOY_EVM_BRANCH" ]]; then
    echo "Patching alloy-rs/evm with branch: $ALLOY_EVM_BRANCH"
    cat >> "$CARGO_TOML" << EOF
alloy-evm = { git = "https://github.com/alloy-rs/evm", branch = "$ALLOY_EVM_BRANCH" }
alloy-op-evm = { git = "https://github.com/alloy-rs/evm", branch = "$ALLOY_EVM_BRANCH" }
EOF
fi

if [[ -n "$OP_ALLOY_BRANCH" ]]; then
    echo "Patching alloy-rs/op-alloy with branch: $OP_ALLOY_BRANCH"
    cat >> "$CARGO_TOML" << EOF
op-alloy-consensus = { git = "https://github.com/alloy-rs/op-alloy", branch = "$OP_ALLOY_BRANCH" }
op-alloy-network = { git = "https://github.com/alloy-rs/op-alloy", branch = "$OP_ALLOY_BRANCH" }
op-alloy-rpc-types = { git = "https://github.com/alloy-rs/op-alloy", branch = "$OP_ALLOY_BRANCH" }
op-alloy-rpc-types-engine = { git = "https://github.com/alloy-rs/op-alloy", branch = "$OP_ALLOY_BRANCH" }
op-alloy-rpc-jsonrpsee = { git = "https://github.com/alloy-rs/op-alloy", branch = "$OP_ALLOY_BRANCH" }
EOF
fi

echo "Done. Patches appended to $CARGO_TOML"
echo ""
echo "Run 'cargo check --workspace --all-features' to verify compilation."
echo "Run 'git checkout $CARGO_TOML' to revert changes."
