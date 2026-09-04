#!/usr/bin/env bash

set -euo pipefail

ALLOW_MODIFIED_LOCKFILE=false
if [[ "${1:-}" == "--allow-modified-lockfile" ]]; then
  ALLOW_MODIFIED_LOCKFILE=true
  shift
fi
SOURCE_DIR="${1:?source directory is required}"

VERIFIER="${CARGO_COOLDOWN_BIN:?cargo-cooldown verifier path is required}"
EXPECTED_SHA256="${CARGO_COOLDOWN_SHA256:?cargo-cooldown verifier digest is required}"
if command -v sha256sum >/dev/null; then
  ACTUAL_SHA256="$(sha256sum "$VERIFIER" | awk '{print $1}')"
else
  ACTUAL_SHA256="$(shasum -a 256 "$VERIFIER" | awk '{print $1}')"
fi
if [[ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]]; then
  echo "cargo-cooldown verifier checksum mismatch" >&2
  exit 1
fi

if [[ "$ALLOW_MODIFIED_LOCKFILE" == "false" ]]; then
  git -C "$SOURCE_DIR" ls-files --error-unmatch Cargo.lock >/dev/null
  git -C "$SOURCE_DIR" diff --quiet HEAD -- Cargo.lock
elif [[ ! -f "$SOURCE_DIR/Cargo.lock" ]]; then
  echo "Cargo.lock does not exist" >&2
  exit 1
fi

(
  cd "$SOURCE_DIR"
  unset COOLDOWN_CACHE_DIR COOLDOWN_CARGO_COMPATIBLE_ACCEPT COOLDOWN_ENFORCEMENT \
    COOLDOWN_FALLBACK_ACCEPT COOLDOWN_LOCKFILE_BASELINE COOLDOWN_NOW \
    COOLDOWN_SKIP_REGISTRIES COOLDOWN_TTL_SECONDS CARGO_REGISTRY_MIN_PUBLISH_AGE
  while IFS= read -r variable; do
    case "$variable" in
      CARGO_REGISTRIES_*_MIN_PUBLISH_AGE) unset "$variable" ;;
    esac
  done < <(compgen -e)
  COOLDOWN_INCOMPATIBLE_PUBLISH_AGE=deny \
  COOLDOWN_LOCKFILE_BASELINE=ignore \
  CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE="7 days" \
    "$VERIFIER" cooldown --workspace --all-features tree --locked --depth 0 >/dev/null
)
