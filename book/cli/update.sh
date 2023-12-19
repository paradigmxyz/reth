#!/usr/bin/env bash
set -eo pipefail

BOOK_ROOT="$(dirname "$(dirname "$0")")"
RETH=${1:-"$(dirname "$BOOK_ROOT")/target/debug/reth"}

cmd=(
  "$(dirname "$0")/help.py"
  --root-dir "$BOOK_ROOT/"
  --root-indentation 2
  --root-summary
  --out-dir "$BOOK_ROOT/cli/"
  "$RETH"
)
echo "Running: $" "${cmd[*]}"
"${cmd[@]}"
