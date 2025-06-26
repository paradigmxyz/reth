#!/usr/bin/env bash
set -eo pipefail

BOOK_ROOT="$(dirname "$(dirname "$0")")"
RETH=${1:-"$(dirname "$BOOK_ROOT")/target/debug/reth"}
VOCS_PAGES_ROOT="$BOOK_ROOT/vocs/docs/pages"
echo "Generating CLI documentation for reth at $RETH"

echo "Using book root: $BOOK_ROOT"
echo "Using vocs pages root: $VOCS_PAGES_ROOT"
cmd=(
  "$(dirname "$0")/help.rs"
  --root-dir "$BOOK_ROOT/"
  --root-indentation 2
  --root-summary
  --verbose
  --out-dir "$VOCS_PAGES_ROOT/cli/"
  "$RETH"
)
echo "Running: $" "${cmd[*]}"
"${cmd[@]}"
