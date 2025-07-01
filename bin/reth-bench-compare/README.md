# reth-bench-compare

Automated tool for comparing reth performance between two git references (branches or tags). This tool completely automates your benchmark workflow, handling git branch/tag switching, compilation, node management, and results comparison.

## Overview

This tool automates the complete workflow you described:

1. Compile reth using `make profiling` on git reference A
2. Run reth with metrics and engine API enabled
3. Run reth-bench for N blocks starting from the current tip
4. Stop reth node gracefully and clean up lock files
5. Unwind to original tip
6. Repeat for git reference B
7. Compare results and generate detailed reports
8. Optionally generate comparison charts using Python script

## Usage

### Basic Command

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref my-optimization \
  --blocks 100
```

**Note**: The JWT secret file is automatically generated at `<datadir>/<chain>/jwt.hex` if not provided. For mainnet with default datadir, this would be `~/.local/share/reth/mainnet/jwt.hex` on Linux.

### Full Command with All Options

```bash
reth-bench-compare \
  --baseline-ref main \
  --feature-ref my-optimization \
  --chain mainnet \
  --datadir ~/chain/reth/data \
  --datadir.static-files ~/chain/reth/static_files \
  --blocks 100 \
  --rpc-url https://reth-ethereum.ithaca.xyz/rpc \
  --jwt-secret ~/chain/reth/data/jwt.hex \
  --output-dir ./benchmark-comparison \
  --metrics-port 5005 \
  --sudo \
  --draw \
  --compile-reth-bench
```

### Using Tags

```bash
reth-bench-compare \
  --baseline-ref v1.4.8 \
  --feature-ref v1.5.0 \
  --blocks 500 \
  --draw
```

## Arguments

| Argument | Description | Default |
|----------|-------------|---------|
| `--baseline-ref` | Git reference (branch or tag) to use as baseline | Required |
| `--feature-ref` | Git reference (branch or tag) to compare against baseline | Required |
| `--jwt-secret` | JWT secret file path | `<datadir>/<chain>/jwt.hex` |
| `--chain` | Chain to use for reth operations | `mainnet` |
| `--datadir` | Reth datadir path (defaults to OS-specific location) | OS default |
| `--datadir.static-files` | Path to store static files | Uses datadir |
| `--blocks` | Number of blocks to benchmark | `100` |
| `--rpc-url` | RPC endpoint for fetching block data | `https://reth-ethereum.ithaca.xyz/rpc` |
| `--output-dir` | Output directory for results | `./benchmark-comparison` |
| `--metrics-port` | Port for reth metrics endpoint | `5005` |
| `--sudo` | Run reth binary with sudo (for elevated privileges) | `false` |
| `--draw` | Generate comparison charts using Python script | `false` |
| `--compile-reth-bench` | Compile reth-bench (by default, only reth is compiled) | `false` |
| `--skip-git-validation` | Skip git working directory validation | `false` |
| `--skip-compilation` | Skip reth compilation (use existing binaries) | `false` |

## Output

The tool generates timestamped output directories with:

```
benchmark-comparison/
└── 20240101_120000/
    ├── baseline_ref/
    │   ├── combined_latency.csv
    │   └── total_gas.csv
    ├── feature_ref/
    │   ├── combined_latency.csv
    │   └── total_gas.csv
    ├── comparison_report.json
    ├── per_block_comparison.csv
    └── latency_comparison.png (if --draw used)
```

### Console Output

The tool prints a comprehensive comparison summary:

```
=== BENCHMARK COMPARISON SUMMARY ===
Timestamp: 2024-01-01 12:00:00 UTC
Baseline: main
Feature:  my-optimization

Performance Changes:
  NewPayload Latency: -15.32%
  FCU Latency:        -2.14%
  Total Latency:      -12.45%
  Gas/Second:         +14.26%
  Blocks/Second:      +13.89%

Baseline Summary:
  Blocks: 100, Gas: 2847291948, Duration: 45.23s
  Avg NewPayload: 387.42ms, Avg FCU: 52.18ms, Avg Total: 439.60ms

Feature Summary:
  Blocks: 100, Gas: 2847291948, Duration: 39.60s
  Avg NewPayload: 327.95ms, Avg FCU: 51.06ms, Avg Total: 379.01ms
```

## Prerequisites

1. **Git Repository**: Must be run from the root of a reth git repository
2. **Clean Working Directory**: Git working directory must be clean (no uncommitted changes)
3. **Reference Availability**: Both baseline and feature git references (branches/tags) must exist
4. **Build Environment**: Must have make and rust toolchain available
5. **reth-bench**: Available in PATH (optionally compiled by the tool)
6. **JWT Secret**: JWT secret file for engine API authentication (auto-generated if not provided)
7. **Python & uv**: Required for chart generation (if using `--draw`)

## Safety Features

- **Git State Protection**: Automatically restores original branch on completion or interruption
- **Process Cleanup**: Handles Ctrl+C gracefully, cleaning up processes and git state
- **Lock File Cleanup**: Automatically removes database and static file locks after node shutdown
- **Validation**: Validates git state, reference existence, and build requirements before starting
- **Error Recovery**: Comprehensive error handling with clear recovery instructions

## Workflow Details

For each git reference, the tool:

1. **Git Operations**:
   - Validates reference exists and working directory is clean
   - Switches to target reference (branch or tag)
   - Handles detached HEAD state for tags

2. **Compilation**:
   - Runs `make profiling` to build optimized reth binary
   - Optionally compiles reth-bench if requested
   - Verifies successful compilation

3. **Node Management**:
   - Starts reth node with metrics and engine API enabled
   - Waits for node RPC to be ready
   - Monitors node health during benchmark

4. **Benchmarking**:
   - Runs reth-bench for specified block range
   - Generates combined_latency.csv and total_gas.csv files
   - Verifies output file generation

5. **Cleanup**:
   - Stops reth node (attempting graceful shutdown)
   - Removes database and static file locks
   - Unwinds node state to original tip
   - Restores original git branch

6. **Comparison** (after both references):
   - Loads and analyzes CSV data from both runs
   - Generates detailed comparison reports
   - Optionally creates visual charts with Python script

## Logging and Output Control

The tool uses reth's standard logging infrastructure with proper log levels:

### Default Output (INFO level)
Shows tool progress and important messages:
```bash
reth-bench-compare --baseline-ref main --feature-ref my-branch ...
```

### Quiet Mode
Suppresses most output, showing only warnings and errors:
```bash
reth-bench-compare --baseline-ref main --feature-ref my-branch ... --quiet
```

### Verbose Mode
Shows detailed subprocess output (compilation, reth node logs, benchmark output):
```bash
# Using verbosity flags
reth-bench-compare --baseline-ref main --feature-ref my-branch ... -vv

# Using RUST_LOG environment variable
RUST_LOG=debug reth-bench-compare --baseline-ref main --feature-ref my-branch ...
```

The verbosity levels are:
- Default: Shows tool progress (INFO level)
- `-v`: Shows INFO level (same as default)
- `-vv`: Shows DEBUG level (includes subprocess output)
- `-vvv`: Shows TRACE level
- `--quiet`: Suppresses output below WARN level

## Example Workflow

```bash
# Start from main branch
git checkout main

# Run comparison between main and optimization branch
reth-bench-compare \
  --baseline-ref main \
  --feature-ref performance-optimization \
  --chain mainnet \
  --blocks 50 \
  --jwt-secret ~/chain/reth/data/jwt.hex \
  --output-dir ./perf-comparison \
  --draw

# Tool will:
# 1. Validate git state and references
# 2. Build and benchmark main reference
# 3. Build and benchmark performance-optimization reference  
# 4. Generate detailed comparison report
# 5. Create visual comparison charts
# 6. Restore to original branch
```

## Integration with Existing Tools

This tool integrates seamlessly with existing reth tooling:

- **Uses reth-bench**: Leverages standard reth-bench for benchmarking
- **Metrics Integration**: Compatible with existing Prometheus/Grafana monitoring
- **CSV Output**: Generates standard CSV files compatible with existing analysis scripts
- **Python Charts**: Automatically uses `compare_newpayload_latency.py` with `--draw` option
- **Standard Logging**: Uses reth's logging infrastructure for consistent output

## Troubleshooting

### Common Issues

1. **"Git working directory is not clean"**
   - Commit or stash your changes before running
   - Use `git status` to see uncommitted changes

2. **"Git reference 'xyz' does not exist"**
   - Ensure reference exists: `git branch -a | grep xyz` or `git tag | grep xyz`
   - Create branch if needed: `git checkout -b xyz`
   - For tags, ensure they're fetched: `git fetch --tags`

3. **"Compilation failed"**
   - Check build environment and dependencies
   - Try manual compilation: `make profiling`
   - Use `-vv` flag to see detailed compilation output

4. **"Node failed to start"**
   - Check if ports are available (default: 8551, 5005)
   - Verify datadir permissions and disk space
   - Ensure JWT secret file exists and is readable
   - Try running with `--sudo` if permission issues

5. **"Chart generation failed"**
   - Ensure `uv` is installed: `curl -LsSf https://astral.sh/uv/install.sh | sh`
   - Python dependencies are managed automatically by uv

### Debug Options

Use debug flags and verbose logging for troubleshooting:

```bash
# Skip validations for testing
reth-bench-compare \
  --baseline-ref main \
  --feature-ref test \
  --jwt-secret ~/chain/reth/data/jwt.hex \
  --skip-git-validation \
  --skip-compilation \
  --blocks 10

# Verbose output to see all subprocess logs
reth-bench-compare \
  --baseline-ref main \
  --feature-ref test \
  --jwt-secret ~/chain/reth/data/jwt.hex \
  --blocks 10 \
  -vv

# Use RUST_LOG for even more detailed logging
RUST_LOG=debug,reth_bench_compare=trace reth-bench-compare \
  --baseline-ref main \
  --feature-ref test \
  --jwt-secret ~/chain/reth/data/jwt.hex \
  --blocks 10
```