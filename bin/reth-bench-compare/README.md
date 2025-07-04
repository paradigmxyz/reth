# reth-bench-compare

Automated performance comparison tool for reth across different git references.

## Overview

Compares reth performance between two git references by:
1. Building reth from each reference
2. Running benchmarks on identical block ranges
3. Generating detailed comparison reports

## Features

- **Automated Workflow**: Handles git operations, compilation, and benchmarking
- **Binary Caching**: Reuses compiled binaries across runs
- **Safety**: Restores original git state on completion or interruption
- **CPU Profiling**: Optional integration with samply profiler
- **Visual Reports**: Generates charts and CSV data for analysis

## Output Structure

```
reth-bench-compare/
├── bin/                    # Cached compiled binaries
├── profiles/               # CPU profiling data (optional)
└── results/                # Benchmark results
    └── TIMESTAMP/          # Timestamped results
        ├── baseline/       # Baseline reference data
        ├── feature/        # Feature reference data
        └── comparison_report.json
```

## Performance Metrics

The tool compares:
- NewPayload latency
- ForkchoiceUpdated latency
- Total latency
- Gas processed per second
- Blocks processed per second

## Prerequisites

- Clean git working directory
- Rust toolchain and make
- Valid git references (branches, tags, or commits)
- JWT secret for engine API (auto-generated if needed)

## Workflow

For each reference:
1. Switch to git reference
2. Build optimized reth binary
3. Start reth node
4. Run benchmarks
5. Stop node and unwind state
6. Generate comparison reports

## Troubleshooting

Use `-vv` for detailed output or check the timestamped log files in the output directory.

For CPU profiling analysis, profiles can be opened in Firefox Profiler at https://profiler.firefox.com/