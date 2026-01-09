# reth-bench-compare

Compare reth performance between two git references (branches, tags, or commits).

## Quick Start

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref my-feature \
  --blocks 100 \
  --chain base \
  --wait-for-persistence
```

## CLI Arguments

### Required Arguments

| Argument | Description | Optional |
|----------|-------------|----------|
| `--baseline-ref <REF>` | Git reference for baseline comparison | ❌ Required |
| `--feature-ref <REF>` | Git reference to compare against baseline | ❌ Required |

### Core Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--blocks <N>` | Number of blocks to benchmark | `100` | ✅ Optional |
| `--chain <CHAIN>` | Chain to benchmark (mainnet, base, etc.) | `mainnet` | ✅ Optional |
| `--datadir <PATH>` | Path to reth data directory | OS-specific | ✅ Optional |
| `--rpc-url <URL>` | RPC endpoint for fetching blocks | Chain default | ✅ Optional |
| `--jwt-secret <PATH>` | JWT secret file path | `<datadir>/<chain>/jwt.hex` | ✅ Optional |
| `--output-dir <PATH>` | Output directory for results | `./reth-bench-compare` | ✅ Optional |

### Wait Mode Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--wait-for-persistence` | Wait for block persistence (recommended) | `false` | ✅ Optional |
| `--persistence-threshold <N>` | Wait after every N+1 blocks | `2` | ✅ Optional |
| `--wait-time <DURATION>` | Fixed delay between blocks (legacy) | None | ✅ Optional |

**Note:** `--wait-time` takes precedence over `--wait-for-persistence`.

### Warmup Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--warmup-blocks <N>` | Blocks to run for cache warmup | Same as `--blocks` | ✅ Optional |
| `--no-clear-cache` | Skip cache clearing before warmup | `false` | ✅ Optional |

### Output Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--draw` | Generate comparison charts (requires Python/uv) | `false` | ✅ Optional |
| `--profile` | Enable CPU profiling with samply | `false` | ✅ Optional |
| `-vvvv` | Verbose logging (debug level) | Info | ✅ Optional |

### Compilation Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--features <FEATURES>` | Rust features for both builds | `jemalloc,asm-keccak` | ✅ Optional |
| `--baseline-features <FEATURES>` | Features for baseline only | Inherits `--features` | ✅ Optional |
| `--feature-features <FEATURES>` | Features for feature build only | Inherits `--features` | ✅ Optional |
| `--rustflags <FLAGS>` | RUSTFLAGS for both builds | `-C target-cpu=native` | ✅ Optional |
| `--baseline-rustflags <FLAGS>` | RUSTFLAGS for baseline only | Inherits `--rustflags` | ✅ Optional |
| `--feature-rustflags <FLAGS>` | RUSTFLAGS for feature only | Inherits `--rustflags` | ✅ Optional |

### Node Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--metrics-port <PORT>` | Port for reth metrics endpoint | `5005` | ✅ Optional |
| `--sudo` | Run reth with elevated privileges | `false` | ✅ Optional |
| `--baseline-args <ARGS>` | Extra args for baseline node | None | ✅ Optional |
| `--feature-args <ARGS>` | Extra args for feature node | None | ✅ Optional |
| `[RETH_ARGS]...` | Args for both nodes (after `--`) | None | ✅ Optional |

### Advanced Options

| Argument | Description | Default | Optional |
|----------|-------------|---------|----------|
| `--skip-git-validation` | Skip git state validation | `false` | ✅ Optional |
| `--disable-startup-sync-state-idle <TARGET>` | Disable idle flag for specific runs | None | ✅ Optional |

## Examples

### Basic Comparison

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref my-optimization \
  --blocks 50
```

### Persistence-Based Benchmarking

```bash
reth-bench-compare \
  --baseline-ref v1.0.0 \
  --feature-ref v1.1.0 \
  --wait-for-persistence \
  --persistence-threshold 2 \
  --blocks 100 \
  --chain base
```

### With Custom RPC and Output

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref pr/12345 \
  --blocks 100 \
  --rpc-url https://base-mainnet.example.com \
  --output-dir ./my-results \
  --draw \
  -vvvv
```

### With Profiling

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref optimization-branch \
  --blocks 50 \
  --profile \
  --draw
```

### Custom Compilation Flags

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref my-feature \
  --baseline-rustflags "-C target-cpu=native" \
  --feature-rustflags "-C target-cpu=native -C lto" \
  --blocks 100
```

## Output

Results are saved in the output directory (default: `./reth-bench-compare/results/<timestamp>/`):

- `comparison_report.json` - Detailed metrics comparison
- `per_block_comparison.csv` - Per-block statistics
- `baseline/` - Baseline benchmark results
  - `combined_latency.csv` - Latency measurements
  - `reth_bench.log` - Benchmark logs
  - `reth_node.log` - Node logs
- `feature/` - Feature benchmark results (same structure)
- `latency_comparison.png` - Visual comparison (if `--draw` used)

## Notes

- **Persistence Mode (Recommended)**: Use `--wait-for-persistence` for realistic benchmarks that account for persistence overhead
- **Legacy Mode**: Use `--wait-time <duration>` for fixed delays (e.g., `--wait-time 100ms`)
- **Warmup**: By default, runs warmup phase with same block count as benchmark to warm caches
- **Profiling**: Requires `samply` installed (`cargo install samply`)
- **Charts**: Requires Python with `uv` package manager
