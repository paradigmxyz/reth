#!/usr/bin/env bash
set -euo pipefail

exec "$(dirname "$0")/compare-witness-recent.py" "$@"
