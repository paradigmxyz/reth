#!/usr/bin/env bash
set -eo pipefail

DOCS_ROOT="$(dirname "$(dirname "$0")")"
RETH=${1:-"$(dirname "$DOCS_ROOT")/target/debug/reth"}
VOCS_PAGES_ROOT="$DOCS_ROOT/vocs/docs/pages"
# Build help.rs against its committed lockfile instead of the newest dependency releases.
CARGO_RESOLVER_LOCKFILE_PATH="$(dirname "$0")/Cargo.lock"
export CARGO_RESOLVER_LOCKFILE_PATH
echo "Generating CLI documentation for reth at $RETH"

echo "Using docs root: $DOCS_ROOT"
echo "Using vocs pages root: $VOCS_PAGES_ROOT"
cmd=(
  "$(dirname "$0")/help.rs"
  --root-dir "$DOCS_ROOT/"
  --root-indentation 2
  --root-summary
  --sidebar
  --verbose
  --out-dir "$VOCS_PAGES_ROOT/cli/"
  "$RETH"
)
echo "Running: $" "${cmd[*]}"
"${cmd[@]}"

