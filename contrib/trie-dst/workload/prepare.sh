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
cargo build --locked --manifest-path "$root/crates/reth-trie-dst/Cargo.toml" --profile dst -j "${CARGO_BUILD_JOBS:-6}"
context=$(mktemp -d)
trap 'rm -rf "$context"' EXIT
# Keep symbols in the original executable for debugging recorded failures.
objcopy --strip-debug "${CARGO_TARGET_DIR:-$root/../../target}/dst/reth-trie-dst" "$context/reth-trie-dst"
cc -O2 -I "$BEDROCK_ROOT/guest" -x c - -o "$context/bedrock-ready" <<'C'
#include "libvmcall.h"
int main(void) {
    vmcall_ready();
    return 0;
}
C
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
