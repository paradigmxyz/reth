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
      if [ "$big_blocks" = true ] || [ "$bal" != false ]; then
        expected='build --locked --profile profiling -p reth-bb'
      else
        expected='build --locked --profile profiling --bin reth'
      fi
      [[ "$(cat "$BUILD_TEST_LOG")" == "$expected" ]]

      node - "$(dirname "$script")/../workflows/bench.yml" "$big_blocks" "$bal" <<'JS'
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const [workflow, bigBlocks, bal] = process.argv.slice(2);
const source = fs.readFileSync(workflow, 'utf8').match(/^\s*const nodeBin = .+;$/m)[0];
const nodeBin = vm.runInNewContext(`${source}\nnodeBin`, { bigBlocks, bal });
assert.equal(nodeBin, bigBlocks === 'true' || bal !== 'false' ? 'reth-bb' : 'reth');
JS
    done
  done
done
echo 'PASS: both arms build the BAL-capable node for all BAL modes'
