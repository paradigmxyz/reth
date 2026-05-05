#!/usr/bin/env bash
set -euo pipefail

LABEL="${1:?label required}"
RUN_DIR="${2:?run dir required}"
GIT_REF="${3:?git ref required}"
NODE_DIR="${4:?node dir required}"

LAST_RUN_START=$(date +%s)
echo "BENCH_LAST_RUN_START=${LAST_RUN_START}" >> "$GITHUB_ENV"
cat > "$BENCH_LABELS_FILE" <<LABELS
{"benchmark_run":"${RUN_DIR}","run_type":"${LABEL}","git_ref":"${GIT_REF}","bench_sha":"${GIT_REF}","benchmark_id":"${BENCH_ID}","run_start_epoch":"${LAST_RUN_START}","reference_epoch":"${BENCH_REFERENCE_EPOCH}"}
LABELS

RUN_SCRIPT=.github/scripts/bench-reth-run.sh
if [ "$BENCH_DRIVER" = "txgen" ]; then
  RUN_SCRIPT=.github/scripts/bench-txgen-run.sh
fi
NODE_BINARY="../${NODE_DIR}/target/profiling/${BENCH_NODE_BIN}"
OUTPUT_DIR="$BENCH_WORK_DIR/${RUN_DIR}"
python3 .github/scripts/bench-run-manifest.py \
  --output-dir "$OUTPUT_DIR" \
  --run-dir "$RUN_DIR" \
  --run-type "$LABEL" \
  --git-ref "$GIT_REF" \
  --node-binary "$NODE_BINARY" \
  --status started
taskset -c 0 "$RUN_SCRIPT" "$LABEL" "$NODE_BINARY" "$OUTPUT_DIR"
python3 .github/scripts/bench-run-manifest.py \
  --output-dir "$OUTPUT_DIR" \
  --run-dir "$RUN_DIR" \
  --run-type "$LABEL" \
  --git-ref "$GIT_REF" \
  --node-binary "$NODE_BINARY" \
  --status completed
