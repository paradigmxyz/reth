#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")" && pwd)
: "${BEDROCK_ROOT:?Set BEDROCK_ROOT to a Bedrock checkout}"
read -r -a docker_cmd <<< "${DOCKER:-docker}"
CARGO_PROFILE_PROFILING_DEBUG_ASSERTIONS=true CARGO_PROFILE_PROFILING_OVERFLOW_CHECKS=true \
  cargo build --locked --manifest-path "$root/crates/reth-trie-dst/Cargo.toml" --profile profiling -j "${CARGO_BUILD_JOBS:-6}"
context=$(mktemp -d)
trap 'rm -rf "$context"' EXIT
# Keep symbols in the original executable for debugging recorded failures.
objcopy --strip-debug "${CARGO_TARGET_DIR:-$root/../../target}/profiling/reth-trie-dst" "$context/reth-trie-dst"
cc -O2 -I "$BEDROCK_ROOT/guest" "$root/workload/ready.c" -o "$context/bedrock-ready"
cp "$root/workload/controller.py" "$context/controller.py"
"${docker_cmd[@]}" build -f "$root/workload/Dockerfile" -t bedrock/reth-trie-dst:latest "$context"
