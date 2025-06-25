# reth-bench-compare

Automated tool for comparing reth performance between two git branches. This tool completely automates your benchmark workflow, handling git branch switching, compilation, node management, and results comparison.

## Overview

This tool automates the complete workflow you described:

1. Compile reth using `make profiling` on branch A
2. Run reth with metrics and engine API enabled
3. Run reth-bench for N blocks starting from the current tip
4. Stop reth node and unwind to original tip
5. Repeat for branch B
6. Compare results using the new baseline comparison feature

## Usage

### Basic Command

```bash
reth-bench-compare \
  --baseline-branch main \
  --feature-branch my-optimization \
  --blocks 100
```

### Full Command with Options

```bash
reth-bench-compare \
  --baseline-branch main \
  --feature-branch my-optimization \
  --datadir ~/.local/share/reth/mainnet \
  --blocks 100 \
  --rpc-url https://reth-ethereum.ithaca.xyz/rpc \
  --jwt-secret ~/.local/share/reth/mainnet/jwt.hex \
  --output-dir ./benchmark-results \
  --metrics-port 5005
```

## Arguments

| Argument | Description | Default |
|----------|-------------|---------|
| `--baseline-branch` | Git branch to use as baseline | Required |
| `--feature-branch` | Git branch to compare against baseline | Required |
| `--datadir` | Reth datadir path | `~/.local/share/reth/mainnet` |
| `--blocks` | Number of blocks to benchmark | `100` |
| `--rpc-url` | RPC endpoint for fetching block data | `https://reth-ethereum.ithaca.xyz/rpc` |
| `--jwt-secret` | JWT secret file path | `{datadir}/jwt.hex` |
| `--output-dir` | Output directory for results | `./benchmark-comparison` |
| `--metrics-port` | Port for reth metrics endpoint | `5005` |
| `--skip-git-validation` | Skip git working directory validation | `false` |
| `--skip-compilation` | Skip reth compilation (use existing binaries) | `false` |

## Output

The tool generates timestamped output directories with:

```
benchmark-comparison/
└── 20240101_120000/
    ├── baseline_branch/
    │   ├── combined_latency.csv
    │   ├── total_gas.csv
    │   └── baseline_comparison.csv (for feature branch)
    ├── feature_branch/
    │   ├── combined_latency.csv
    │   ├── total_gas.csv
    │   └── baseline_comparison.csv
    ├── comparison_report.json
    └── per_block_comparison.csv
```

### Console Output

The tool prints a comprehensive comparison summary:

```
=== BENCHMARK COMPARISON SUMMARY ===
Timestamp: 20240101_120000
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
3. **Branch Availability**: Both baseline and feature branches must exist
4. **Build Environment**: Must have make and rust toolchain available
5. **reth-bench**: The enhanced reth-bench with baseline comparison support

## Safety Features

- **Git State Protection**: Automatically restores original branch on completion or interruption
- **Process Cleanup**: Handles Ctrl+C gracefully, cleaning up processes and git state
- **Validation**: Validates git state, branch existence, and build requirements before starting
- **Error Recovery**: Comprehensive error handling with clear recovery instructions

## Workflow Details

For each branch, the tool:

1. **Git Operations**:
   - Validates branch exists and working directory is clean
   - Switches to target branch
   - Optionally pulls latest changes

2. **Compilation**:
   - Runs `make profiling` to build optimized reth binary
   - Verifies successful compilation

3. **Node Management**:
   - Starts reth node with metrics and engine API enabled
   - Waits for node to sync to current tip
   - Monitors node health during benchmark

4. **Benchmarking**:
   - Runs reth-bench for specified block range
   - Uses baseline comparison for feature branch
   - Verifies output file generation

5. **Cleanup**:
   - Stops reth node gracefully
   - Unwinds node state to original tip
   - Restores git branch

## Example Workflow

```bash
# Start from main branch
git checkout main

# Run comparison between main and optimization branch
reth-bench-compare \
  --baseline-branch main \
  --feature-branch performance-optimization \
  --blocks 50 \
  --output-dir ./perf-comparison

# Tool will:
# 1. Validate git state and branches
# 2. Build and benchmark main branch
# 3. Build and benchmark performance-optimization branch  
# 4. Generate detailed comparison report
# 5. Restore to main branch
```

## Integration with Existing Tools

This tool leverages and enhances your existing workflow:

- **Uses reth-bench**: Leverages the enhanced reth-bench with `--baseline` support
- **Metrics Integration**: Compatible with existing Prometheus/Grafana monitoring
- **CSV Output**: Generates CSV files compatible with existing analysis scripts
- **Python Script**: Output can still be used with the existing `compare_newpayload_latency.py` script

## Troubleshooting

### Common Issues

1. **"Git working directory is not clean"**
   - Commit or stash your changes before running
   - Use `git status` to see uncommitted changes

2. **"Branch 'xyz' does not exist"**
   - Ensure branch exists: `git branch -a | grep xyz`
   - Create branch if needed: `git checkout -b xyz`

3. **"Compilation failed"**
   - Check build environment and dependencies
   - Try manual compilation: `make profiling`

4. **"Node failed to start"**
   - Check if ports are available (default: 8551, 5005)
   - Verify datadir permissions and disk space

### Debug Options

Use `--skip-git-validation` and `--skip-compilation` for debugging:

```bash
reth-bench-compare \
  --baseline-branch main \
  --feature-branch test \
  --skip-git-validation \
  --skip-compilation \
  --blocks 10
```