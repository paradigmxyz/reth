#!/usr/bin/env bash
# Restore the last successful benchmark commit from an Actions artifact.
# Usage: bench-state.sh owner/repo hourly|nightly|release
# Requires GH_TOKEN with actions:read. State is scoped to GITHUB_REF_NAME (main
# outside Actions). Missing/expired state bootstraps; API/corruption errors fail.
set -euo pipefail

repo=${1:?expected owner/repo}
mode=${2:?expected benchmark mode}
case "$mode" in hourly|nightly|release) ;; *) echo "Invalid benchmark mode: $mode" >&2; exit 1 ;; esac
name="bench-state-$mode"
branch=${GITHUB_REF_NAME:-main}

# Filter before downloading: dispatches on test branches must not change main's
# baseline. Pagination matters when hourly state accumulates over its retention.
artifacts=$(gh api --paginate --slurp "repos/$repo/actions/artifacts?name=$name&per_page=100")
candidates=$(jq -r --arg name "$name" --arg branch "$branch" '
  [.[].artifacts[] | select(.name == $name and .expired == false
    and .workflow_run.head_branch == $branch
    and .workflow_run.head_repository_id == .workflow_run.repository_id)]
  | sort_by(.id) | reverse | .[] | [.id, .workflow_run.id] | @tsv
' <<< "$artifacts")

while IFS=$'\t' read -r artifact_id run_id; do
  [[ -n "$artifact_id" ]] || continue
  run=$(gh api "repos/$repo/actions/runs/$run_id")
  if ! jq -e --arg branch "$branch" '
    .path == ".github/workflows/bench-scheduled.yml" and .head_branch == $branch
    and .status == "completed" and .conclusion == "success"
    and (.event == "schedule" or .event == "workflow_dispatch")
  ' <<< "$run" >/dev/null; then
    continue
  fi
  scratch=$(mktemp -d)
  trap 'rm -rf "$scratch"' EXIT
  gh api "repos/$repo/actions/artifacts/$artifact_id/zip" > "$scratch/state.zip"
  value=$(unzip -p "$scratch/state.zip" feature-ref)
  if [[ ! "$value" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Invalid commit SHA in $name artifact $artifact_id" >&2
    exit 1
  fi
  echo "Restored $name from run $run_id" >&2
  printf '%s\n' "$value"
  exit 0
done <<< "$candidates"
