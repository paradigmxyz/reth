#!/bin/bash
# Heavy Performance Benchmarks for Reth/Tempo
#
# Based on #eng-perf Slack discussions identifying key bottlenecks:
# - update_history_indices: 26% of persist time
# - write_trie_updates: 25.4%
# - write_trie_changesets: 24.2%
# - Execution cache contention under high throughput
#
# Run this on an idle reth box (check with: reth-box-status)
#
# Usage: ./scripts/bench-heavy.sh [output_dir]

set -euo pipefail

OUTPUT_DIR="${1:-./benchmark-results}"
mkdir -p "$OUTPUT_DIR"

echo "============================================="
echo "Heavy Performance Benchmarks for Reth/Tempo"
echo "============================================="
echo "Output directory: $OUTPUT_DIR"
echo "Started at: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# Build with optimizations
echo "[1/5] Building benchmarks with release profile..."
cargo build --release --benches -p reth-engine-tree -p reth-trie-parallel -p reth-trie-sparse

# Run execution cache benchmarks
echo ""
echo "[2/5] Running execution cache benchmarks..."
echo "  - Cache hit rate simulations (45%, 78%, 90%, 95%)"
echo "  - Concurrent access contention (1-32 threads)"
echo "  - Burst insert patterns (2K-50K entries)"
echo "  - TIP-20 transfer patterns"
echo "  - Pre-warming effectiveness"
cargo bench -p reth-engine-tree --bench execution_cache -- --save-baseline heavy-cache 2>&1 | tee "$OUTPUT_DIR/execution_cache.log"

# Run heavy persistence benchmarks
echo ""
echo "[3/5] Running heavy persistence benchmarks..."
echo "  - write_hashed_state with 500-5000 accounts"
echo "  - Accumulated block persistence (75-250 blocks)"
echo "  - State root calculation after persistence"
echo "  - History indices insertion"
cargo bench -p reth-engine-tree --bench heavy_persistence -- --save-baseline heavy-persist 2>&1 | tee "$OUTPUT_DIR/heavy_persistence.log"

# Run parallel state root benchmarks
echo ""
echo "[4/5] Running parallel state root benchmarks..."
echo "  - Sync vs Parallel root (3K-30K accounts)"
echo "  - Incremental updates (5-50 sequential)"
echo "  - Large storage tries (1K-10K slots)"
cargo bench -p reth-trie-parallel --bench heavy_root -- --save-baseline heavy-root 2>&1 | tee "$OUTPUT_DIR/heavy_root.log"

# Run existing state root task benchmarks
echo ""
echo "[5/5] Running state root task benchmarks..."
cargo bench -p reth-engine-tree --bench state_root_task -- --save-baseline state-root 2>&1 | tee "$OUTPUT_DIR/state_root_task.log"

echo ""
echo "============================================="
echo "Benchmarks completed at: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Results saved to: $OUTPUT_DIR"
echo ""
echo "To compare baselines later:"
echo "  cargo bench -p reth-engine-tree --bench execution_cache -- --baseline heavy-cache"
echo ""
echo "To generate HTML reports:"
echo "  open target/criterion/*/report/index.html"
echo "============================================="
