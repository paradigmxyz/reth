#!/usr/bin/env bash
set -euo pipefail

script="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/bench-txgen-build.sh"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export BUILD_TEST_LOG="$scratch/build-args"

cargo() {
  printf '%s\n' "$*" > "$BUILD_TEST_LOG"
}
rustup() { :; }
export -f cargo rustup

for arm in baseline feature; do
  for bal in false true baseline feature; do
    for big_blocks in false true; do
      BENCH_BAL="$bal" BENCH_BIG_BLOCKS="$big_blocks" BENCH_TRACY=off \
        bash "$script" "$arm" "$scratch" test-ref > /dev/null 2>&1
      if [ "$big_blocks" = true ]; then
        expected='build --locked --profile profiling -p reth-bb'
      else
        expected='build --locked --profile profiling --bin reth'
      fi
      [[ "$(cat "$BUILD_TEST_LOG")" == "$expected" ]]
    done
  done
done
echo 'PASS: both arms build the selected node for all BAL modes'
