#!/usr/bin/env bash

set -euo pipefail

REPOSITORY="${1:?repository URL is required}"
REVISION="${2:?revision is required}"
shift 2

if [[ ! "$REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "revision must be a full commit SHA" >&2
  exit 1
fi

SOURCE_DIR="${RUNNER_TEMP:?RUNNER_TEMP is required}/cargo-tool-${REVISION}"
git clone --filter=blob:none --no-checkout "$REPOSITORY" "$SOURCE_DIR"
git -C "$SOURCE_DIR" fetch --depth 1 origin "$REVISION"
git -C "$SOURCE_DIR" checkout --detach "$REVISION"

.github/scripts/verify-cargo-lock-cooldown.sh "$SOURCE_DIR"

INSTALL_ARGS=()
for package in "$@"; do
  INSTALL_ARGS+=(--package "$package")
done
cargo install --path "$SOURCE_DIR" --locked "${INSTALL_ARGS[@]}"
