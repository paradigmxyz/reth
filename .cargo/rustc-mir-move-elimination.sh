#!/usr/bin/env bash
set -euo pipefail
exec rustup run nightly-2026-10-23 rustc -Zmir-move-elimination "$@"
