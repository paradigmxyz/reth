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
"$root/build.sh"
read -r -a docker_cmd <<< "${DOCKER:-docker}"
mkdir -p "$RUN_DIR"
cp "$root/workload/compose.yaml" "$RUN_DIR/compose.yaml"
cp "$root/workload/controller.py" "$RUN_DIR/controller.py"
"${docker_cmd[@]}" save bedrock/reth-trie-dst:latest > "$RUN_DIR/images.raw.tar"
python3 "$BEDROCK_ROOT/scripts/dst/oci_archive.py" normalize "$RUN_DIR/images.raw.tar" -o "$RUN_DIR/images.tar"
rm "$RUN_DIR/images.raw.tar"
printf '%s\n' "$RUN_DIR/images.tar" > "$RUN_DIR/images.path"
