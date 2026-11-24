#!/usr/bin/env bash
# Hill-climb score for level-batched sparse trie hashing.
#
# Builds the arena_root bench, runs it RUNS times at K (default 5000) with both hashing
# strategies interleaved per block, and prints the median over runs of each run's median
# root-hashing time per block, and the speedup of LevelBatched over DepthFirst.
#
# Usage: crates/trie/sparse/benches/hillclimb.sh [extra cargo args, e.g. --features mbkeccak]
# Env: RUNS (default 5), K (default 5000), BLOCKS (default 30), SLOTS, ACCOUNTS, TOKENS.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
RUNS=${RUNS:-5}
K=${K:-5000}
BLOCKS=${BLOCKS:-30}
FEATURES="alloy-primitives/asm-keccak"
cargo bench -p reth-trie-sparse --bench arena_root --features "$FEATURES" "$@" --no-run 2>&1 |
    grep -E '^error' && exit 1
BIN=$(cargo bench -p reth-trie-sparse --bench arena_root --features "$FEATURES" "$@" --no-run 2>&1 |
    sed -n 's/.*Executable benches\/arena_root.rs (\(.*\))/\1/p')
OUT=$(mktemp)
for _ in $(seq "$RUNS"); do
    KS=$K BLOCKS=$BLOCKS MODES=depth_first,level_batched "$BIN" 2>/dev/null | grep -v '^K' >>"$OUT"
done
awk -v k="$K" '
    $1 == k { hash[$2] = hash[$2] " " $6; total[$2] = total[$2] " " $7 }
    function med(s,    a, n, i, j, t) {
        n = split(s, a, " ")
        for (i = 2; i <= n; i++) for (j = i; j > 1 && a[j - 1] + 0 > a[j] + 0; j--) { t = a[j]; a[j] = a[j - 1]; a[j - 1] = t }
        return a[int((n + 1) / 2)]
    }
    END {
        b = med(hash["DepthFirst"]); n = med(hash["LevelBatched"])
        bt = med(total["DepthFirst"]); nt = med(total["LevelBatched"])
        printf "K=%d hash_ms depth_first=%.3f level_batched=%.3f speedup=%.2fx | total_ms %.3f -> %.3f (%.2fx)\n", k, b, n, b / n, bt, nt, bt / nt
        printf "SCORE %.0f ns/block (level_batched hash, K=%d), speedup %.2fx\n", n * 1e6, k, b / n
    }' "$OUT"
rm -f "$OUT"
