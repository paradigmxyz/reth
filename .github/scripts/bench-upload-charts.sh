#!/usr/bin/env bash
# Upload benchmark PNGs using the endpoint behind gh --attach.
# Usage: bash bench-upload-charts.sh [--dry-run] owner/repo charts-directory
# Requires gh authentication with a user token that can write to the repository.
set -euo pipefail

dry_run=false
if [[ "${1:-}" == --dry-run ]]; then
  dry_run=true
  shift
fi
if [[ $# != 2 ]]; then
  echo "Usage: $0 [--dry-run] owner/repo charts-directory" >&2
  exit 1
fi
repo=$1
directory=$2
shopt -s nullglob
charts=("$directory"/*.png)
if (( ${#charts[@]} == 0 )); then
  echo "No PNG charts found in $directory" >&2
  exit 1
fi
# Validate the whole batch before uploading anything.
for chart in "${charts[@]}"; do
  if [[ ! -f "$chart" || ! -s "$chart" ]] || (( $(wc -c < "$chart") > 10485760 )); then
    echo "Expected a nonempty PNG no larger than 10 MiB: $chart" >&2
    exit 1
  fi
  if [[ "$(od -An -tx1 -N8 "$chart" | tr -d ' \n')" != 89504e470d0a1a0a ]]; then
    echo "Invalid PNG: $chart" >&2
    exit 1
  fi
done

if $dry_run; then
  printf '%q ' gh api "repos/$repo" --jq .id
  printf '\n'
  repository_id=REPOSITORY_ID
else
  repository_id=$(gh api "repos/$repo" --jq .id)
  [[ "$repository_id" =~ ^[1-9][0-9]*$ ]]
  printf '\n\n### Charts\n\n'
fi
for chart in "${charts[@]}"; do
  name=${chart##*/}
  command=(gh api https://uploads.github.com/user-attachments/assets
    --method POST -H 'Content-Type: application/octet-stream'
    -H 'Accept: application/vnd.github+json' --input "$chart"
    -f "name=$name" -f content_type=image/png -F "repository_id=$repository_id")
  if $dry_run; then
    printf '%q ' "${command[@]}"
    printf '\n'
    continue
  fi
  # Print successful URLs immediately; never automatically retry an upload.
  response=$("${command[@]}")
  url=$(jq -er '.url | select(type == "string" and startswith("https://github.com/user-attachments/assets/"))' <<< "$response")
  case "$name" in
    latency_throughput.png) label='Latency, Throughput & Diff' ;;
    wait_breakdown.png) label='Wait Time Breakdown' ;;
    gas_vs_latency.png) label='Gas vs Latency' ;;
    *) label='Benchmark chart' ;;
  esac
  printf '<details><summary>%s</summary>\n\n![%s](%s)\n\n</details>\n\n' "$label" "$label" "$url"
done
