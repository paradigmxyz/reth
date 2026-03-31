#!/usr/bin/env bash
#
# Resolves baseline and feature refs for scheduled benchmark runs.
#
# Supports two modes:
#   nightly — Queries the latest successful scheduled docker.yml run via
#             GitHub API to find the nightly Docker image commit. Compares
#             with the last successful feature ref to detect staleness.
#   hourly  — Compares origin/main HEAD against the last successfully
#             benchmarked commit (falls back to HEAD~1 on first run).
#             Checks for in-progress sibling runs to avoid overlap.
#
# Usage: bench-scheduled-refs.sh <force> <mode>
#   force — "true" to run even if no new commit (bypass skip logic)
#   mode  — "nightly" or "hourly"
#
# Outputs (via GITHUB_OUTPUT):
#   baseline-ref    — commit SHA for baseline
#   feature-ref     — commit SHA for feature
#   should-skip     — "true" if no new commit since last run or sibling in progress
#   is-stale        — "true" if latest nightly build is >24h old (nightly only)
#   stale-age-hours — age of the nightly build in hours (nightly only)
#   nightly-created — ISO timestamp of the nightly build (nightly only)
#
# Reads:
#   state/nightly-last-feature-ref  (nightly, from decofe/reth-bench-charts repo)
#   state/hourly-last-feature-ref   (hourly, from decofe/reth-bench-charts repo)
#
# Requires: gh (GitHub CLI), jq, date, git (hourly mode), curl, DEREK_TOKEN env
set -euo pipefail

FORCE="${1:-false}"
MODE="${2:-nightly}"
REPO="${GITHUB_REPOSITORY:-paradigmxyz/reth}"

echo "Mode: $MODE, Force: $FORCE"

# ==========================================================================
# Hourly mode: compare origin/main HEAD vs HEAD~1
# ==========================================================================
if [ "$MODE" = "hourly" ]; then

  # --- Step 1: Resolve feature ref from git ---
  echo "::group::Resolving hourly refs from git"
  git fetch origin main --depth=2 --quiet
  FEATURE_REF=$(git rev-parse origin/main)
  echo "Feature (HEAD): $FEATURE_REF"
  echo "::endgroup::"

  # --- Step 2: Check for in-progress sibling runs ---
  echo "::group::Checking for in-progress sibling runs"
  CURRENT_RUN_ID="${GITHUB_RUN_ID:-0}"
  IN_PROGRESS=$(gh run list \
    -R "$REPO" \
    --workflow=bench-scheduled.yml \
    --status=in_progress \
    --json databaseId \
    --jq "[.[] | select(.databaseId != $CURRENT_RUN_ID)] | length")

  SHOULD_SKIP="false"
  if [ "$IN_PROGRESS" -gt 0 ]; then
    echo "::warning::Previous bench run still in progress ($IN_PROGRESS sibling run(s) found). Skipping."
    SHOULD_SKIP="true"
    # Output a flag so the workflow can send a Slack alert
    echo "long-running=true" >> "$GITHUB_OUTPUT"
  else
    echo "No in-progress sibling runs"
    echo "long-running=false" >> "$GITHUB_OUTPUT"
  fi
  echo "::endgroup::"

  # --- Step 3: Read last successful feature ref from charts repo ---
  echo "::group::Reading persisted state"
  LAST_FEATURE_REF=""
  STATE_URL="https://raw.githubusercontent.com/decofe/reth-bench-charts/state/state/hourly-last-feature-ref"
  if RAW=$(curl -sfL -H "Authorization: token ${DEREK_TOKEN}" "$STATE_URL"); then
    LAST_FEATURE_REF=$(echo "$RAW" | tr -d '[:space:]')
    echo "Previous feature ref: $LAST_FEATURE_REF"
  else
    echo "No persisted state found (first run)"
  fi
  echo "::endgroup::"

  # --- Step 4: Determine baseline and skip logic ---
  echo "::group::Resolving baseline and skip logic"
  if [ "$SHOULD_SKIP" = "true" ]; then
    BASELINE_REF=$(git rev-parse origin/main~1)
    echo "Already marked skip (sibling in progress)"
  elif [ -z "$LAST_FEATURE_REF" ]; then
    # First run: no previous state, fall back to HEAD~1
    BASELINE_REF=$(git rev-parse origin/main~1)
    echo "First run — using HEAD~1 as baseline"
  elif [ "$LAST_FEATURE_REF" = "$FEATURE_REF" ]; then
    BASELINE_REF="$LAST_FEATURE_REF"
    if [ "$FORCE" = "true" ] || [ "$FORCE" = "--force" ]; then
      echo "No new commits on main, but force=true — running anyway"
    else
      SHOULD_SKIP="true"
      echo "No new commits on main since last run — will skip"
    fi
  else
    # Normal case: use last benchmarked commit as baseline
    BASELINE_REF="$LAST_FEATURE_REF"
    echo "New commit(s) on main detected — comparing against last benchmarked commit"
  fi

  echo "Baseline: $BASELINE_REF"
  echo "Feature:  $FEATURE_REF"
  echo "Skip:     $SHOULD_SKIP"
  echo "::endgroup::"

  # --- Step 5: Write outputs ---
  {
    echo "baseline-ref=$BASELINE_REF"
    echo "feature-ref=$FEATURE_REF"
    echo "should-skip=$SHOULD_SKIP"
    echo "is-stale=false"
    echo "stale-age-hours=0"
    echo "nightly-created="
  } >> "$GITHUB_OUTPUT"
  exit 0
fi

# ==========================================================================
# Nightly mode: query latest Docker nightly build (original logic)
# ==========================================================================

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

# --- Step 3: Read last successful feature ref from charts repo ---
echo "::group::Reading persisted state"
LAST_FEATURE_REF=""
STATE_URL="https://raw.githubusercontent.com/decofe/reth-bench-charts/state/state/nightly-last-feature-ref"
if RAW=$(curl -sfL -H "Authorization: token ${DEREK_TOKEN}" "$STATE_URL"); then
  LAST_FEATURE_REF=$(echo "$RAW" | tr -d '[:space:]')
  echo "Previous feature ref: $LAST_FEATURE_REF"
else
  echo "No persisted state found (first run)"
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
