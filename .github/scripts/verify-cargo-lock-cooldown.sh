#!/usr/bin/env bash

set -euo pipefail

SOURCE_DIR="${1:?source directory is required}"

command -v cargo-cooldown >/dev/null
git -C "$SOURCE_DIR" ls-files --error-unmatch Cargo.lock >/dev/null
git -C "$SOURCE_DIR" diff --quiet HEAD -- Cargo.lock

(
  cd "$SOURCE_DIR"
  unset COOLDOWN_CACHE_DIR COOLDOWN_FALLBACK_ACCEPT COOLDOWN_NOW \
    COOLDOWN_SKIP_REGISTRIES COOLDOWN_TTL_SECONDS CARGO_REGISTRY_MIN_PUBLISH_AGE
  CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE="7 days" \
    cargo-cooldown cooldown --workspace --all-features tree --locked --depth 0 >/dev/null
)
