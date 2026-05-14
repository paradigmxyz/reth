#!/usr/bin/env bash
# Updates the reth-bench PR comment with current status via the GitHub API.
#
# Usage: bench-update-status.sh "Running benchmark: feature (1/2)..."
#
# Reads from environment:
#   BENCH_COMMENT_ID  – GitHub comment ID to update
#   BENCH_GH_TOKEN    – GitHub token for API auth
#   BENCH_ACTOR       – User who triggered the benchmark
#   BENCH_JOB_URL     – URL to the Actions job page
#   BENCH_CONFIG      – Config line (blocks, warmup, refs)
#   GITHUB_REPOSITORY – owner/repo

set -euo pipefail

STATUS="$1"

if [ -z "${BENCH_COMMENT_ID:-}" ] || [ -z "${BENCH_GH_TOKEN:-}" ]; then
  exit 0
fi

BODY=$(printf 'cc @%s\n\n🚀 Benchmark started! [View job](%s)\n\n⏳ **Status:** %s\n\n%s' \
  "${BENCH_ACTOR:-}" "${BENCH_JOB_URL:-}" "$STATUS" "${BENCH_CONFIG:-}")

PAYLOAD=$(jq -n --arg body "$BODY" '{body: $body}')

curl -sS -X PATCH \
  "https://api.github.com/repos/${GITHUB_REPOSITORY}/issues/comments/${BENCH_COMMENT_ID}" \
  -H "Authorization: token ${BENCH_GH_TOKEN}" \
  -H "Accept: application/vnd.github+json" \
  -d "$PAYLOAD" > /dev/null || echo "Warning: failed to update PR comment status"
