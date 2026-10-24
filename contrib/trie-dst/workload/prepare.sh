#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
: "${RUN_DIR:?Bedrock must supply RUN_DIR}"
: "${BEDROCK_ROOT:?Bedrock must supply BEDROCK_ROOT}"
case "$*" in
  --workload-config\ *) test "$#" = 2 ;;
  *) echo 'expected --workload-config FILE' >&2; exit 1 ;;
esac
# Validate before building; parse-args is also used by the host driver.
python3 -c 'import json,sys; print(json.dumps({"config_file":sys.argv[1],"args":[]}))' "$2" | "$root/workload/controller.py" parse-args - >/dev/null
read -r -a docker_cmd <<< "${DOCKER:-docker}"
cache=${BEDROCK_DST_BUILD_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/reth-trie-dst}
sccache=${BEDROCK_DST_SCCACHE_DIR:-$cache/sccache}
mkdir -p "$cache" "$sccache" "$RUN_DIR"
cache=$(cd "$cache" && pwd)
sccache=$(cd "$sccache" && pwd)
# Share the lock with Bedrock's cache pruning.
exec 9>"$cache/build.lock"
flock 9
context=$(mktemp -d)
build_started=$SECONDS
trap 'printf "prepare_secs=%s\n" "$((SECONDS - build_started))" >> "$RUN_DIR/cache-info.env"; rm -rf "$context"' EXIT
printf 'cache_dir=%s\nsccache_dir=%s\n' "$cache" "$sccache" > "$RUN_DIR/cache-info.env"
printf '%s\n' bedrock/reth-trie-dst-builder:rust1.96-sccache0.14 bedrock/reth-trie-dst:latest > "$RUN_DIR/docker-images"
"${docker_cmd[@]}" build -f - -t bedrock/reth-trie-dst-builder:rust1.96-sccache0.14 "$context" <<'DOCKERFILE'
FROM rust:1.96-bookworm@sha256:19817ead3289c8c631c73df281e18b59b172f6a31f4f563290f69cddd06c30e9
RUN apt-get update && apt-get install -y --no-install-recommends clang libclang-dev mold \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install sccache --version 0.14.0 --locked
DOCKERFILE
# Stable paths let fresh CI checkouts reuse compiler results.
"${docker_cmd[@]}" run --rm -i --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$(cd "$root/../.." && pwd),dst=/workspace,readonly" \
  --mount "type=bind,src=$cache,dst=/cache" \
  --mount "type=bind,src=$sccache,dst=/cache/sccache" \
  --mount "type=bind,src=$context,dst=/out" \
  --mount "type=bind,src=$RUN_DIR,dst=/run-info" \
  --mount "type=bind,src=$BEDROCK_ROOT/guest,dst=/bedrock-guest,readonly" \
  --mount "type=bind,src=$BEDROCK_ROOT/scripts/dst/rust-coverage.sh,dst=/bedrock-rust-coverage.sh,readonly" \
  -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}" \
  -e SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}" \
  -e DST_CMD="${DST_CMD:-campaign}" \
  -w /workspace bedrock/reth-trie-dst-builder:rust1.96-sccache0.14 bash -seu <<'BUILD'
export HOME=/cache CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target/reth-trie-dst
export SCCACHE_DIR=/cache/sccache RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0
export CC='sccache gcc' CXX='sccache g++' RUSTFLAGS='-C link-arg=-fuse-ld=mold'
mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$SCCACHE_DIR"
touch "$CARGO_HOME" "$CARGO_TARGET_DIR"
sccache --zero-stats >/dev/null
trap 'sccache --show-stats > /run-info/sccache.txt; cat /run-info/sccache.txt' EXIT
# Keep the DST profile: it inherits profiling and enables debug assertions and
# overflow checks for the harness. Do not switch these builds to profiling.
if [ "${DST_CMD:-campaign}" = fuzz ]; then
  source /bedrock-rust-coverage.sh
  bedrock_rust_coverage_build /bedrock-guest /out reth-trie-dst dst --locked -p reth-trie-dst > /run-info/coverage-binary-path
  binary=$(cat /run-info/coverage-binary-path)
  cargo rustc --locked -p reth-trie-dst --profile dst --target "$(rustc -vV | sed -n 's/^host: //p')" -- -C target-cpu=native
  bedrock_rust_coverage_check "$binary" --version
else
  cargo build --locked -p reth-trie-dst --profile dst
  binary="$CARGO_TARGET_DIR/dst/reth-trie-dst"
fi
printf 'cargo_secs=%s\n' "$SECONDS" >> /run-info/cache-info.env
# Keep symbols in the cached executable for debugging recorded failures.
objcopy --strip-debug "$binary" /run-info/reth-trie-dst
objcopy --strip-debug "$binary" /out/reth-trie-dst
sccache gcc -O2 -I /bedrock-guest -x c - -o /out/bedrock-ready <<'C'
#include "libvmcall.h"
int main(void) {
    vmcall_ready();
    return 0;
}
C
BUILD
rm -f "$context/libfeedback.o" "$context/libpcguard.o" "$context/rt.a"
cp "$root/workload/controller.py" "$context/controller.py"
"${docker_cmd[@]}" build -f - -t bedrock/reth-trie-dst:latest "$context" <<'DOCKERFILE'
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends python3 libstdc++6 && rm -rf /var/lib/apt/lists/*
COPY reth-trie-dst bedrock-ready controller.py /usr/local/bin/
ENTRYPOINT ["/bin/sh", "-c", "/usr/local/bin/bedrock-ready && exec sleep infinity"]
DOCKERFILE
mkdir -p "$RUN_DIR"
cp "$root/workload/compose.yaml" "$RUN_DIR/compose.yaml"
cp "$root/workload/controller.py" "$RUN_DIR/controller.py"
"${docker_cmd[@]}" save bedrock/reth-trie-dst:latest > "$RUN_DIR/images.raw.tar"
python3 "$BEDROCK_ROOT/scripts/dst/oci_archive.py" normalize "$RUN_DIR/images.raw.tar" -o "$RUN_DIR/images.tar"
rm "$RUN_DIR/images.raw.tar"
printf '%s\n' "$RUN_DIR/images.tar" > "$RUN_DIR/images.path"
