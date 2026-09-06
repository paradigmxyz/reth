#!/usr/bin/env bash
set -euo pipefail

samply_root="${RUNNER_TEMP}/proof-samply-${GITHUB_RUN_ID}"
samply_revision=dac609073bd4a41c1c1e4e4b12b617fd41b5d9ac
mkdir -p "$samply_root"
git init -q "$samply_root/src"
git -C "$samply_root/src" fetch --depth 1 https://github.com/DaniPopes/samply "$samply_revision"
git -C "$samply_root/src" checkout --detach FETCH_HEAD

# Event bursts can fill the buffer while the profiler unwinds earlier samples.
python3 - "$samply_root/src/samply/src/linux/perf_event.rs" <<'PYTHON'
from pathlib import Path
import sys
path = Path(sys.argv[1])
source = path.read_text()
old = "const STACK_COUNT_PER_BUFFER: u32 = 32;"
assert source.count(old) == 1
path.write_text(source.replace(old, "const STACK_COUNT_PER_BUFFER: u32 = 1024;"))
PYTHON

CARGO_TARGET_DIR="$samply_root/target" cargo build \
  --manifest-path "$samply_root/src/Cargo.toml" --release --locked -p samply
printf 'BENCH_SAMPLY_BIN=%s\n' "$samply_root/target/release/samply" >> "$GITHUB_ENV"
"$samply_root/target/release/samply" --version
