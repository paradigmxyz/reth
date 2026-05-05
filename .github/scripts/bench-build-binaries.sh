#!/usr/bin/env bash
set -euo pipefail

BASELINE_DIR="$(cd ../reth-baseline && pwd)"
FEATURE_DIR="$(cd ../reth-feature && pwd)"

if [ "$BENCH_DRIVER" = "txgen" ]; then
  .github/scripts/bench-txgen-install.sh
  BUILD_SCRIPT=.github/scripts/bench-txgen-build.sh
else
  BUILD_SCRIPT=.github/scripts/bench-reth-build.sh
fi

"$BUILD_SCRIPT" baseline "${BASELINE_DIR}" "${BASELINE_REF:?BASELINE_REF must be set}" &
PID_BASELINE=$!
"$BUILD_SCRIPT" feature "${FEATURE_DIR}" "${FEATURE_REF:?FEATURE_REF must be set}" &
PID_FEATURE=$!

FAIL=0
wait $PID_BASELINE || FAIL=1
wait $PID_FEATURE || FAIL=1
if [ $FAIL -ne 0 ]; then
  echo "::error::One or more build tasks failed"
  exit 1
fi
