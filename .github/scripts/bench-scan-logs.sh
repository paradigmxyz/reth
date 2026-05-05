#!/usr/bin/env bash
set -euo pipefail

ERRORS_FILE="$BENCH_WORK_DIR/errors.md"
found=false
for run_dir in baseline-1 feature-1 feature-2 baseline-2; do
  LOG="$BENCH_WORK_DIR/$run_dir/node.log"
  if [ ! -f "$LOG" ]; then continue; fi

  panics=$(grep -c -E 'panicked at' "$LOG" || true)
  errors=$(grep -c ' ERROR ' "$LOG" || true)

  if [ "$panics" -gt 0 ] || [ "$errors" -gt 0 ]; then
    if [ "$found" = false ]; then
      printf '### ⚠️ Node Errors\n\n' >> "$ERRORS_FILE"
      found=true
    fi
    printf '<details><summary><b>%s</b>: %d panic(s), %d error(s)</summary>\n\n' "$run_dir" "$panics" "$errors" >> "$ERRORS_FILE"
    if [ "$panics" -gt 0 ]; then
      printf '**Panics:**\n```\n' >> "$ERRORS_FILE"
      grep -E 'panicked at' "$LOG" | head -10 >> "$ERRORS_FILE"
      printf '```\n' >> "$ERRORS_FILE"
    fi
    if [ "$errors" -gt 0 ]; then
      printf '**Errors (first 20):**\n```\n' >> "$ERRORS_FILE"
      grep ' ERROR ' "$LOG" | head -20 >> "$ERRORS_FILE"
      printf '```\n' >> "$ERRORS_FILE"
    fi
    printf '\n</details>\n\n' >> "$ERRORS_FILE"
  fi
done
