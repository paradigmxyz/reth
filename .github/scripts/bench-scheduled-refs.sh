#!/usr/bin/env bash
#
# Resolves baseline and feature refs for nightly regression benchmark runs.
#
# Queries the latest successful scheduled docker.yml run via GitHub API
# to find the commit that built the nightly Docker image. Compares with
# the last successful feature ref (from GH Actions cache) to determine
# baseline, detect staleness, and decide whether to skip.
#
# Usage: bench-nightly-refs.sh [--force]
#
# Outputs (via GITHUB_OUTPUT):
#   baseline-ref    — commit SHA for baseline
#   feature-ref     — commit SHA for feature (current nightly)
#   should-skip     — "true" if no new nightly since last run
#   is-stale        — "true" if latest nightly build is >24h old
#   stale-age-hours — age of the nightly build in hours (only if stale)
#   nightly-created — ISO timestamp of the nightly build
#
# Reads:
#   .nightly-state/last-feature-ref (from GH Actions cache, may not exist)
#
# Requires: gh (GitHub CLI), jq, date
set -euo pipefail

FORCE="${1:-false}"
REPO="${GITHUB_REPOSITORY:-paradigmxyz/reth}"

# --- Step 1: Query latest successful scheduled docker.yml run ---
echo "::group::Querying latest nightly docker build"

RUNS_JSON=$(gh run list \
  -R "$REPO" \
  --workflow=docker.yml \
  --event=schedule \
  --status=completed \
  --limit 5 \
  --json headSha,createdAt,conclusion)

# Find the most recent successful run
LATEST=$(echo "$RUNS_JSON" | jq -r '[.[] | select(.conclusion == "success")] | first // empty')

if [ -z "$LATEST" ]; then
  echo "::error::No successful scheduled docker.yml run found in the last 5 runs"
  echo "Runs found: $RUNS_JSON"
  exit 1
fi

FEATURE_REF=$(echo "$LATEST" | jq -r '.headSha')
CREATED_AT=$(echo "$LATEST" | jq -r '.createdAt')
echo "Latest nightly commit: $FEATURE_REF"
echo "Built at: $CREATED_AT"
echo "::endgroup::"

# --- Step 2: Staleness check ---
echo "::group::Checking staleness"
NOW_EPOCH=$(date +%s)
# Handle both GNU date (-d) and BSD date (-j -f) for cross-platform compat
CREATED_EPOCH=$(date -d "$CREATED_AT" +%s 2>/dev/null || \
  date -j -f "%Y-%m-%dT%H:%M:%SZ" "$CREATED_AT" +%s 2>/dev/null || \
  date -j -f "%Y-%m-%dT%T%z" "$CREATED_AT" +%s 2>/dev/null || \
  { echo "::error::Cannot parse date: $CREATED_AT"; exit 1; })

AGE_SECONDS=$(( NOW_EPOCH - CREATED_EPOCH ))
AGE_HOURS=$(( AGE_SECONDS / 3600 ))
IS_STALE="false"

if [ "$AGE_HOURS" -gt 24 ]; then
  IS_STALE="true"
  echo "::warning::STALE NIGHTLY: Build is ${AGE_HOURS}h old (>24h threshold)"
  echo "This indicates the nightly docker build failed — no new image was produced"
else
  echo "Nightly build age: ${AGE_HOURS}h (within 24h threshold)"
fi
echo "::endgroup::"

# --- Step 3: Read last successful feature ref from cache ---
echo "::group::Reading cached state"
LAST_FEATURE_REF=""
STATE_FILE=".nightly-state/last-feature-ref"
if [ -f "$STATE_FILE" ]; then
  LAST_FEATURE_REF=$(tr -d '[:space:]' < "$STATE_FILE")
  echo "Previous feature ref: $LAST_FEATURE_REF"
else
  echo "No cached state found (first run)"
fi
echo "::endgroup::"

# --- Step 4: Determine baseline and skip logic ---
echo "::group::Resolving refs"
SHOULD_SKIP="false"
BASELINE_REF="$FEATURE_REF"  # default for first run

if [ "$IS_STALE" = "true" ]; then
  # Stale = error path, don't skip (will alert and fail downstream)
  SHOULD_SKIP="false"
  BASELINE_REF="${LAST_FEATURE_REF:-$FEATURE_REF}"
  echo "Stale nightly detected — will alert and fail"
elif [ -z "$LAST_FEATURE_REF" ]; then
  # First run: baseline = feature (self-comparison to establish baseline)
  BASELINE_REF="$FEATURE_REF"
  echo "First run — will benchmark nightly against itself to establish baseline"
elif [ "$LAST_FEATURE_REF" = "$FEATURE_REF" ]; then
  # No new nightly since last successful run
  if [ "$FORCE" = "true" ] || [ "$FORCE" = "--force" ]; then
    echo "No new nightly, but force=true — running anyway"
    BASELINE_REF="$LAST_FEATURE_REF"
  else
    SHOULD_SKIP="true"
    echo "No new nightly since last run — will skip"
  fi
else
  # Normal case: new nightly available
  BASELINE_REF="$LAST_FEATURE_REF"
  echo "New nightly detected"
fi

echo "Baseline: $BASELINE_REF"
echo "Feature:  $FEATURE_REF"
echo "Skip:     $SHOULD_SKIP"
echo "Stale:    $IS_STALE"
echo "::endgroup::"

# --- Step 5: Write outputs ---
{
  echo "baseline-ref=$BASELINE_REF"
  echo "feature-ref=$FEATURE_REF"
  echo "should-skip=$SHOULD_SKIP"
  echo "is-stale=$IS_STALE"
  echo "stale-age-hours=$AGE_HOURS"
  echo "nightly-created=$CREATED_AT"
} >> "$GITHUB_OUTPUT"
