# reth-bench-compare

Compare reth performance between two git references.

## Usage

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref my-feature \
  --blocks 100 \
  --wait-for-persistence
```

## Arguments

| Argument | Description | Default | Required |
|----------|-------------|---------|----------|
| `--baseline-ref <REF>` | Git reference for baseline | - | Yes |
| `--feature-ref <REF>` | Git reference to compare | - | Yes |
| `--blocks <N>` | Number of blocks to benchmark | `100` | No |
| `--chain <CHAIN>` | Chain to benchmark | `mainnet` | No |
| `--datadir <PATH>` | Data directory path | OS-specific | No |
| `--rpc-url <URL>` | RPC endpoint for block data | Chain default | No |
| `--output-dir <PATH>` | Output directory | `./reth-bench-compare` | No |
| `--wait-for-persistence` | Wait for block persistence | `false` | No |
| `--persistence-threshold <N>` | Wait after every N+1 blocks | `2` | No |
| `--wait-time <DURATION>` | Fixed delay (legacy) | - | No |
| `--warmup-blocks <N>` | Cache warmup blocks | Same as `--blocks` | No |
| `--draw` | Generate charts (needs Python/uv) | `false` | No |
| `--profile` | Enable CPU profiling (needs samply) | `false` | No |
| `-vvvv` | Debug logging | Info | No |
| `--features <FEATURES>` | Extra Rust features for both builds | - | No |
| `--rustflags <FLAGS>` | RUSTFLAGS for both builds | `-C target-cpu=native` | No |
| `--baseline-features <FEATURES>` | Features for baseline only | Inherits `--features` | No |
| `--feature-features <FEATURES>` | Features for feature only | Inherits `--features` | No |
| `--baseline-rustflags <FLAGS>` | RUSTFLAGS for baseline only | Inherits `--rustflags` | No |
| `--feature-rustflags <FLAGS>` | RUSTFLAGS for feature only | Inherits `--rustflags` | No |
| `--baseline-args <ARGS>` | Extra args for baseline node | - | No |
| `--feature-args <ARGS>` | Extra args for feature node | - | No |
| `--metrics-port <PORT>` | Metrics endpoint port | `5005` | No |
| `--sudo` | Run with elevated privileges | `false` | No |

## Output

Results in `./reth-bench-compare/results/<timestamp>/`:
- `comparison_report.json` - Metrics comparison
- `per_block_comparison.csv` - Per-block statistics
- `baseline/` and `feature/` - Individual run results
- `latency_comparison.png` - Chart (if `--draw` used)
