#!/usr/bin/env bash
#
# Downloads a Reth snapshot from a MinIO manifest into the schelk scratch volume
# and promotes it as the schelk baseline. The later prerun step promotes again
# after gas-bump/funding.
#
# Required env:
#   SCHELK_MOUNT
#   BENCHMARKOOR_BIN
#   BENCHMARKOOR_SUITE
#   BENCHMARKOOR_CONTEXT
#   BENCHMARKOOR_FORK
#   BENCHMARKOOR_TEST_TYPE
#   BENCHMARKOOR_METADATA_ROOT
#   BENCHMARKOOR_CACHE
#   BENCHMARKOOR_SNAPSHOT
#   BENCH_RETH_BINARY
#
# Optional env:
#   BENCHMARKOOR_SNAPSHOT_MC_ROOT  MinIO alias/root used when
#                                  BENCHMARKOOR_SNAPSHOT is a bare prefix.
set -euo pipefail

: "${SCHELK_MOUNT:?SCHELK_MOUNT must be set}"
: "${BENCHMARKOOR_BIN:?BENCHMARKOOR_BIN must be set}"
: "${BENCHMARKOOR_SUITE:?BENCHMARKOOR_SUITE must be set}"
: "${BENCHMARKOOR_CONTEXT:?BENCHMARKOOR_CONTEXT must be set}"
: "${BENCHMARKOOR_FORK:?BENCHMARKOOR_FORK must be set}"
: "${BENCHMARKOOR_TEST_TYPE:?BENCHMARKOOR_TEST_TYPE must be set}"
: "${BENCHMARKOOR_METADATA_ROOT:?BENCHMARKOOR_METADATA_ROOT must be set}"
: "${BENCHMARKOOR_CACHE:?BENCHMARKOOR_CACHE must be set}"
: "${BENCHMARKOOR_SNAPSHOT:?BENCHMARKOOR_SNAPSHOT must be set}"
: "${BENCH_RETH_BINARY:?BENCH_RETH_BINARY must be set}"

DATADIR="${SCHELK_MOUNT}/datadir"
SNAPSHOT_DIR="${BENCHMARKOOR_CACHE}/snapshots"
mkdir -p "$SNAPSHOT_DIR"

trim_slashes() {
  printf '%s' "$1" | sed -E 's:/*$::'
}

copy_source_to_file() {
  local source="$1"
  local dest="$2"

  if [ -f "$source" ]; then
    cp "$source" "$dest"
  elif [[ "$source" =~ ^https?:// ]]; then
    curl -fsSL --retry 3 --retry-delay 5 "$source" -o "$dest"
  else
    mc cp "$source" "$dest"
  fi
}

source_exists() {
  local source="$1"
  if [ -f "$source" ]; then
    return 0
  fi
  if [[ "$source" =~ ^https?:// ]]; then
    curl -fsI --retry 2 --retry-delay 2 "$source" >/dev/null 2>&1
    return $?
  fi
  mc stat "$source" >/dev/null 2>&1
}

mc_alias_url() {
  local alias="$1"
  mc alias export "$alias" 2>/dev/null | jq -r '.url // empty'
}

mc_manifest_base_url() {
  local manifest_object="$1"
  local alias rest alias_url

  alias="${manifest_object%%/*}"
  rest="${manifest_object#*/}"
  alias_url="$(mc_alias_url "$alias")"
  if [ -z "$alias_url" ] || [ "$rest" = "$manifest_object" ]; then
    return 1
  fi

  printf '%s/%s\n' "$(trim_slashes "$alias_url")" "$(dirname "$rest")"
}

resolve_manifest_object() {
  local source="$1"
  local root="${BENCHMARKOOR_SNAPSHOT_MC_ROOT:-minio}"
  local candidate found

  if [ -d "$source" ] && [ -f "$(trim_slashes "$source")/manifest.json" ]; then
    printf '%s\n' "$(trim_slashes "$source")/manifest.json"
    return 0
  fi

  if [[ "$source" =~ ^https?:// ]]; then
    if [[ "$(basename "$source")" == "manifest.json" ]]; then
      candidate="$source"
    else
      candidate="$(trim_slashes "$source")/manifest.json"
    fi
    if source_exists "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
    return 1
  fi

  if [[ "$(basename "$source")" == "manifest.json" ]] && source_exists "$source"; then
    printf '%s\n' "$source"
    return 0
  fi

  candidate="$(trim_slashes "$source")/manifest.json"
  if source_exists "$candidate"; then
    printf '%s\n' "$candidate"
    return 0
  fi

  if [[ "$source" != */* ]]; then
    found="$(
      mc find "$root" --name manifest.json 2>/dev/null |
        grep -F "$source" |
        sort |
        tail -n 1 || true
    )"
    if [ -n "$found" ]; then
      printf '%s\n' "$found"
      return 0
    fi
  fi

  return 1
}

network="${BENCHMARKOOR_SUITE%%/*}"
block="${BENCHMARKOOR_SUITE#*/}"
suite_slug="$(
  printf '%s-%s-%s-%s-%s' \
    "$network" "$block" "$BENCHMARKOOR_CONTEXT" "$BENCHMARKOOR_FORK" "$BENCHMARKOOR_TEST_TYPE" |
    sed -E 's/[^A-Za-z0-9_-]+/-/g; s/^-+//; s/-+$//'
)"

sudo schelk recover -y --kill || sudo schelk full-recover -y || true
sudo schelk mount -y

if ! manifest_object="$(resolve_manifest_object "$BENCHMARKOOR_SNAPSHOT")"; then
  echo "::error::Could not find snapshot manifest for BENCHMARKOOR_SNAPSHOT=${BENCHMARKOOR_SNAPSHOT}"
  echo "Pass a MinIO manifest path, a MinIO prefix containing manifest.json, or an HTTP(S) manifest URL."
  exit 1
fi

manifest_raw="${SNAPSHOT_DIR}/${suite_slug}-manifest.raw.json"
manifest_path="${SNAPSHOT_DIR}/${suite_slug}-manifest.json"
copy_source_to_file "$manifest_object" "$manifest_raw"

manifest_base_url=""
if [[ "$manifest_object" =~ ^https?:// ]]; then
  manifest_base_url="$(dirname "$manifest_object")"
elif [[ "$manifest_object" == */* ]]; then
  manifest_base_url="$(mc_manifest_base_url "$manifest_object" || true)"
fi

if [ -n "$manifest_base_url" ]; then
  jq --arg base "$manifest_base_url" '.base_url = $base' "$manifest_raw" > "$manifest_path"
else
  cp "$manifest_raw" "$manifest_path"
fi

sudo rm -rf "$DATADIR"
sudo mkdir -p "$DATADIR"
sudo chown -R "$(id -u):$(id -g)" "$DATADIR"

"$BENCH_RETH_BINARY" download \
  --manifest-path "$manifest_path" \
  -y \
  --minimal \
  --datadir "$DATADIR"

"$BENCHMARKOOR_BIN" \
  --suite "$BENCHMARKOOR_SUITE" \
  --context "$BENCHMARKOOR_CONTEXT" \
  --fork "$BENCHMARKOOR_FORK" \
  --test-type "$BENCHMARKOOR_TEST_TYPE" \
  --metadata-root "$BENCHMARKOOR_METADATA_ROOT" \
  --cache-dir "$BENCHMARKOOR_CACHE" \
  genesis download \
  --datadir "$DATADIR"

if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
  echo "::error::Manifest download did not produce expected db/ and static_files/ directories"
  ls -la "$DATADIR" || true
  exit 1
fi

echo "Downloaded benchmarkoor snapshot from manifest: ${manifest_object}"

sync
sudo schelk promote -y --kill
sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'

echo "Promoted benchmarkoor snapshot into schelk baseline"
