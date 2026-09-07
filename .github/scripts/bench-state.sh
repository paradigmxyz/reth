#!/usr/bin/env bash
# Read/write the last successful benchmark commit in repository Actions variables.
# Usage: bench-state.sh get|set owner/repo BENCH_<SERIES>_LAST_FEATURE_REF [sha]
set -euo pipefail

action=${1:?expected get or set}
repo=${2:?expected owner/repo}
name=${3:?expected variable name}
if [[ ! "$name" =~ ^BENCH_[A-Z0-9_]+_LAST_FEATURE_REF$ ]]; then
  echo "Invalid benchmark state variable: $name" >&2
  exit 1
fi
export GH_TOKEN="${BENCH_STATE_TOKEN:-${GH_TOKEN:-}}"

case "$action" in
  get)
    # Listing distinguishes an absent variable from an authentication/API failure.
    value=$(gh api --paginate "repos/$repo/actions/variables?per_page=100" \
      --jq ".variables[] | select(.name == \"$name\") | .value")
    ;;
  set)
    value=${4:?expected commit SHA}
    ;;
  *)
    echo "Expected get or set" >&2
    exit 1
    ;;
esac
if [[ -n "$value" && ! "$value" =~ ^[0-9a-f]{40}$ ]] || [[ "$action" == set && -z "$value" ]]; then
  echo "Invalid commit SHA in $name" >&2
  exit 1
fi
if [[ "$action" == set ]]; then
  gh variable set "$name" --repo "$repo" --body "$value"
else
  printf '%s\n' "$value"
fi
