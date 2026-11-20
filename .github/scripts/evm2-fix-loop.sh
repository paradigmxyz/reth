#!/usr/bin/env bash
#
# Runs a cargo check and, while it fails, asks Amp to fix the code (Update evm2 workflow).
# Writes passed=true or passed=false to $GITHUB_OUTPUT. Never fails, so later steps still publish
# partial work.
#
# Usage: evm2-fix-loop.sh <check command> <amp prompt>
set -uo pipefail

CHECK="${1:?check command is required}"
PROMPT="${2:?prompt is required}"
MAX_ATTEMPTS=10
# Two loops run per job; 45 minutes each keeps both, plus the builds, inside its 150-minute timeout.
DEADLINE=$((SECONDS + 2700))

run_check() {
  echo "::group::$CHECK"
  # Word splitting of CHECK into a command and its arguments is intended.
  # shellcheck disable=SC2086
  $CHECK 2>&1
  local status=$?
  echo "::endgroup::"
  return "$status"
}

if run_check; then
  echo "passed=true" >> "$GITHUB_OUTPUT"
  exit 0
fi

if ! command -v amp-run > /dev/null; then
  echo "::warning::Amp is not available; leaving the failure for a person to fix"
  echo "passed=false" >> "$GITHUB_OUTPUT"
  exit 0
fi

attempt=0
while [ "$attempt" -lt "$MAX_ATTEMPTS" ] && [ "$SECONDS" -lt "$DEADLINE" ]; do
  attempt=$((attempt + 1))
  echo "::group::Amp attempt $attempt"
  amp-run --dangerously-allow-all -x "$PROMPT" || echo "::warning::Amp attempt $attempt failed; checking the tree anyway"
  echo "::endgroup::"
  if run_check; then
    echo "Passed after Amp attempt $attempt"
    echo "passed=true" >> "$GITHUB_OUTPUT"
    exit 0
  fi
done

echo "::error::Still failing after $attempt Amp attempts / $((SECONDS / 60))m; publishing best effort"
echo "passed=false" >> "$GITHUB_OUTPUT"
