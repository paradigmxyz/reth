#!/usr/bin/env bash
set -euo pipefail

prepare_source_dir() {
  local dir="$1"
  local ref="$2"

  if [ -d "$dir" ]; then
    git -C "$dir" reset --hard HEAD
    git -C "$dir" clean -fdx
    git -C "$dir" fetch origin "$ref"
  else
    git clone . "$dir"
  fi

  git -C "$dir" checkout --force "$ref"
}

prepare_source_dir ../reth-baseline "${BASELINE_REF:?BASELINE_REF must be set}"
prepare_source_dir ../reth-feature "${FEATURE_REF:?FEATURE_REF must be set}"
