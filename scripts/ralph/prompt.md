# RocksDB vs MDBX Benchmark Execution

You are executing a performance benchmarking task for reth storage backends.

## Context

This benchmark compares MDBX (default) vs RocksDB storage performance for historical tables:
- TransactionHashNumbers
- AccountsHistory  
- StoragesHistory

Read the detailed plan at: docs/benchmarking/ROCKSDB_BENCHMARK_PLAN.md

## Environment Variables Required

Before starting, check these environment variables exist (or set defaults):
- `HOODI_DATADIR_MDBX` - Path to MDBX Hoodi datadir
- `HOODI_DATADIR_ROCKS` - Path to RocksDB Hoodi datadir
- `RPC_URL` - Archive node RPC URL for block data (e.g., http://localhost:8545)
- `START_BLOCK` - Block to start benchmark from

## Your Task

1. Read `prd.json` to find the current task (highest priority where `passes: false`)
2. Read `progress.txt` to understand what previous iterations learned
3. Execute the acceptance criteria for that task
4. Update `prd.json` to set `passes: true` if all criteria are met
5. Append your progress to `progress.txt`

## Important Notes

- Use `./target/release/reth` and `./target/release/reth-bench` (build first if needed)
- Results go in `./benchmark-results/` directory
- For long-running commands, monitor progress and capture output
- Stop reth nodes gracefully with SIGTERM (kill, not kill -9)
- If a task requires manual intervention or external resources not available, mark it as blocked in notes

## Completion Signal

When ALL stories have `passes: true`, output:
```
<promise>COMPLETE</promise>
```

Otherwise, complete your current story and end normally.
