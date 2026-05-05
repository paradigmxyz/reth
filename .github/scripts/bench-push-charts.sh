#!/usr/bin/env bash
set -euo pipefail

PR_NUMBER="${BENCH_PR:-0}"
CHART_DIR="pr/${PR_NUMBER}/${RUN_ID}"
CHARTS_REPO="https://x-access-token:${DEREK_TOKEN}@github.com/decofe/reth-bench-charts.git"

TMP_DIR=$(mktemp -d)
if git clone --depth 1 "${CHARTS_REPO}" "${TMP_DIR}" 2>/dev/null; then
  true
else
  git init "${TMP_DIR}"
  git -C "${TMP_DIR}" remote add origin "${CHARTS_REPO}"
fi

mkdir -p "${TMP_DIR}/${CHART_DIR}"
cp "$BENCH_WORK_DIR"/charts/*.png "${TMP_DIR}/${CHART_DIR}/"
git -C "${TMP_DIR}" add "${CHART_DIR}"
if git -C "${TMP_DIR}" diff --cached --quiet; then
  echo "Charts for ${CHART_DIR} are already present, skipping push"
  echo "sha=$(git -C "${TMP_DIR}" rev-parse HEAD)" >> "$GITHUB_OUTPUT"
  rm -rf "${TMP_DIR}"
  exit 0
fi
git -C "${TMP_DIR}" -c user.name="github-actions" -c user.email="github-actions@github.com" \
  commit -m "bench charts for PR #${PR_NUMBER} run ${RUN_ID}"

for attempt in 1 2 3 4 5; do
  if git -C "${TMP_DIR}" push origin HEAD:main; then
    break
  fi
  if [ "$attempt" -eq 5 ]; then
    echo "::error::Failed to push charts after ${attempt} attempts"
    rm -rf "${TMP_DIR}"
    exit 1
  fi
  sleep "$attempt"
  git -C "${TMP_DIR}" fetch origin main
  git -C "${TMP_DIR}" rebase origin/main
done

echo "sha=$(git -C "${TMP_DIR}" rev-parse HEAD)" >> "$GITHUB_OUTPUT"
rm -rf "${TMP_DIR}"
