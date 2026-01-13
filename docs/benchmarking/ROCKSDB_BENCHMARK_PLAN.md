# RocksDB vs MDBX Performance Benchmarking Plan

This document outlines the benchmarking methodology to compare MDBX vs RocksDB storage backend performance on Hoodi network.

## Overview

**Objective**: Measure performance impact of RocksDB integration for historical tables (TransactionHashNumbers, AccountsHistory, StoragesHistory) compared to MDBX-only baseline.

**Test Parameters**:
- Network: Hoodi testnet
- Blocks: 1000 blocks via `reth-bench new-payload-fcu`
- Metrics: Prometheus → Grafana dashboard
- Tracing: Jaeger for detailed spans

## Prerequisites

### 1. Build reth with RocksDB support

```bash
cd /home/yk/tempo/full_rocks

# Build with RocksDB feature enabled
cargo build --release -p reth -p reth-bench \
    --features "jemalloc asm-keccak rocksdb"
```

### 2. Prepare Hoodi Data Directory

You need two separate data directories - one for MDBX baseline and one for RocksDB test:

```bash
# Ensure you have a synced Hoodi node
# Create copies for benchmarking
export HOODI_DATADIR_MDBX="/data/hoodi-mdbx-bench"
export HOODI_DATADIR_ROCKS="/data/hoodi-rocks-bench"

# Copy existing data (or sync from scratch)
cp -r /path/to/existing/hoodi-datadir $HOODI_DATADIR_MDBX
cp -r /path/to/existing/hoodi-datadir $HOODI_DATADIR_ROCKS
```

### 3. Start Observability Stack

#### Prometheus & Grafana (existing setup)

```bash
cd /home/yk/tempo/reth-grafana/monitoring

# Update prometheus.yml to scrape the benchmark node
# Add new job for benchmark reth instance
cat >> prometheus.yml << 'EOF'

  - job_name: 'reth-bench-mdbx'
    static_configs:
      - targets: ['localhost:9100']
        labels:
          instance: 'reth-bench-mdbx'
          storage: 'mdbx'

  - job_name: 'reth-bench-rocks'
    static_configs:
      - targets: ['localhost:9101']
        labels:
          instance: 'reth-bench-rocks'
          storage: 'rocksdb'
EOF

# Restart Prometheus to pick up changes
docker-compose restart prometheus
```

#### Jaeger for Tracing

```bash
docker run --rm --name jaeger \
    -p 16686:16686 \
    -p 4317:4317 \
    -p 4318:4318 \
    -p 5778:5778 \
    -p 9411:9411 \
    cr.jaegertracing.io/jaegertracing/jaeger:2.11.0
```

Jaeger UI will be available at: http://localhost:16686

## Benchmark Execution

### Step 1: Unwind to Starting Block

Before each benchmark run, unwind the node to a consistent starting point:

```bash
# Determine start block (e.g., 1000 blocks before current head)
START_BLOCK=<your_start_block>

# Unwind MDBX node
./target/release/reth stage unwind to-block $START_BLOCK \
    --datadir $HOODI_DATADIR_MDBX \
    --chain hoodi

# Unwind RocksDB node  
./target/release/reth stage unwind to-block $START_BLOCK \
    --datadir $HOODI_DATADIR_ROCKS \
    --chain hoodi
```

### Step 2: Run MDBX Baseline Benchmark

```bash
# Terminal 1: Start reth with MDBX (no RocksDB flags)
./target/release/reth node \
    --chain hoodi \
    --datadir $HOODI_DATADIR_MDBX \
    --metrics localhost:9100 \
    --authrpc.port 8551 \
    --authrpc.jwtsecret /path/to/jwt.hex \
    --http \
    --http.port 8545 \
    --ws \
    --ws.port 8546 \
    2>&1 | tee mdbx_reth.log

# Terminal 2: Run reth-bench for 1000 blocks
./target/release/reth-bench new-payload-fcu \
    --rpc-url http://<archive-node>:8545 \
    --engine-rpc-url http://localhost:8551 \
    --jwt-secret /path/to/jwt.hex \
    --from $START_BLOCK \
    --to $((START_BLOCK + 1000)) \
    --output mdbx_benchmark.csv \
    --wait-for-persistence \
    2>&1 | tee mdbx_bench.log
```

### Step 3: Run RocksDB Benchmark

```bash
# Terminal 1: Start reth with RocksDB enabled
./target/release/reth node \
    --chain hoodi \
    --datadir $HOODI_DATADIR_ROCKS \
    --metrics localhost:9101 \
    --authrpc.port 8552 \
    --authrpc.jwtsecret /path/to/jwt.hex \
    --http \
    --http.port 8547 \
    --ws \
    --ws.port 8548 \
    --storage.transaction-hash-numbers-in-rocksdb \
    --storage.accounts-history-in-rocksdb \
    --storage.storages-history-in-rocksdb \
    2>&1 | tee rocks_reth.log

# Terminal 2: Run reth-bench for 1000 blocks
./target/release/reth-bench new-payload-fcu \
    --rpc-url http://<archive-node>:8545 \
    --engine-rpc-url http://localhost:8552 \
    --jwt-secret /path/to/jwt.hex \
    --from $START_BLOCK \
    --to $((START_BLOCK + 1000)) \
    --output rocks_benchmark.csv \
    --wait-for-persistence \
    2>&1 | tee rocks_bench.log
```

## Key Metrics to Collect

### 1. RocksDB-Specific Metrics

These are exposed by the new RocksDB provider:

| Metric | Description |
|--------|-------------|
| `rocksdb_provider_calls_total{table,operation}` | Total RocksDB operations (get/put/delete/batch-write) |
| `rocksdb_provider_duration_seconds{table,operation}` | Operation latency histogram |

Tables: `TransactionHashNumbers`, `AccountsHistory`, `StoragesHistory`, `Batch`
Operations: `get`, `put`, `delete`, `batch-write`

### 2. Execution Metrics

| Metric | Description |
|--------|-------------|
| `sync_execution_gas_processed_total` | Total gas processed |
| `sync_execution_gas_per_second` | Instantaneous gas throughput |
| `sync_execution_execution_histogram` | Block execution time distribution |
| `sync_execution_execution_duration` | Current block execution time |

### 3. Stage Metrics

| Metric | Description |
|--------|-------------|
| `sync_checkpoint{stage}` | Block number checkpoint per stage |
| `sync_total_elapsed{stage}` | Total time per stage |
| `sync_entities_processed{stage}` | Entities processed per stage |

Key stages for RocksDB: `TransactionLookup`, `IndexAccountHistory`, `IndexStorageHistory`

### 4. State Provider Metrics

| Metric | Description |
|--------|-------------|
| `state_provider_account_fetch_histogram` | Account fetch latency |
| `state_provider_storage_fetch_histogram` | Storage fetch latency |
| `state_provider_code_fetch_histogram` | Bytecode fetch latency |

### 5. Engine API Metrics

| Metric | Description |
|--------|-------------|
| `engine_api_new_payload_latency` | newPayload call latency |
| `engine_api_forkchoice_updated_latency` | forkchoiceUpdated latency |

### 6. Database Table Sizes

| Metric | Description |
|--------|-------------|
| `db_table_size{table}` | Size of each MDBX table in bytes |
| `db_table_entries{table}` | Number of entries per table |

## Grafana Dashboard

Import the following dashboard to visualize the comparison:

### Dashboard JSON

Save to `/home/yk/tempo/reth-grafana/monitoring/grafana/provisioning/dashboards/rocksdb-benchmark.json`

```json
{
  "title": "RocksDB vs MDBX Benchmark",
  "uid": "rocksdb-benchmark",
  "panels": [
    {
      "title": "Gas Throughput (Ggas/s)",
      "type": "timeseries",
      "targets": [
        {
          "expr": "sync_execution_gas_per_second / 1e9",
          "legendFormat": "{{storage}}"
        }
      ]
    },
    {
      "title": "Block Execution Time",
      "type": "timeseries",
      "targets": [
        {
          "expr": "sync_execution_execution_duration",
          "legendFormat": "{{storage}}"
        }
      ]
    },
    {
      "title": "RocksDB Operation Latency",
      "type": "heatmap",
      "targets": [
        {
          "expr": "rate(rocksdb_provider_duration_seconds_bucket[1m])",
          "legendFormat": "{{table}}-{{operation}}"
        }
      ]
    },
    {
      "title": "RocksDB Operations/sec",
      "type": "timeseries",
      "targets": [
        {
          "expr": "rate(rocksdb_provider_calls_total[1m])",
          "legendFormat": "{{table}}-{{operation}}"
        }
      ]
    },
    {
      "title": "Stage Elapsed Time",
      "type": "timeseries",
      "targets": [
        {
          "expr": "sync_total_elapsed{stage=~\"TransactionLookup|IndexAccountHistory|IndexStorageHistory\"}",
          "legendFormat": "{{storage}}-{{stage}}"
        }
      ]
    },
    {
      "title": "MDBX Table Sizes",
      "type": "bargauge",
      "targets": [
        {
          "expr": "db_table_size{table=~\"TransactionHashNumbers|AccountsHistory|StoragesHistory\"}",
          "legendFormat": "{{table}}"
        }
      ]
    }
  ]
}
```

## Analysis Script

Create a Python script to compare the benchmark outputs:

```python
#!/usr/bin/env python3
"""Compare MDBX vs RocksDB benchmark results."""

import pandas as pd
import matplotlib.pyplot as plt
from pathlib import Path

def load_benchmark(csv_path: str) -> pd.DataFrame:
    """Load reth-bench CSV output."""
    df = pd.read_csv(csv_path)
    return df

def compare_benchmarks(mdbx_csv: str, rocks_csv: str, output_dir: str):
    """Generate comparison charts."""
    mdbx = load_benchmark(mdbx_csv)
    rocks = load_benchmark(rocks_csv)
    
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)
    
    # Gas throughput comparison
    fig, ax = plt.subplots(figsize=(12, 6))
    ax.plot(mdbx['block_number'], mdbx['gas_per_second'] / 1e9, label='MDBX', alpha=0.7)
    ax.plot(rocks['block_number'], rocks['gas_per_second'] / 1e9, label='RocksDB', alpha=0.7)
    ax.set_xlabel('Block Number')
    ax.set_ylabel('Gas Throughput (Ggas/s)')
    ax.set_title('Gas Throughput: MDBX vs RocksDB')
    ax.legend()
    ax.grid(True)
    plt.savefig(output_path / 'gas_throughput.png', dpi=150)
    
    # Latency comparison
    fig, ax = plt.subplots(figsize=(12, 6))
    ax.plot(mdbx['block_number'], mdbx['latency_ms'], label='MDBX', alpha=0.7)
    ax.plot(rocks['block_number'], rocks['latency_ms'], label='RocksDB', alpha=0.7)
    ax.set_xlabel('Block Number')
    ax.set_ylabel('Latency (ms)')
    ax.set_title('Block Processing Latency: MDBX vs RocksDB')
    ax.legend()
    ax.grid(True)
    plt.savefig(output_path / 'latency.png', dpi=150)
    
    # Summary statistics
    print("\n=== Benchmark Summary ===\n")
    print("MDBX:")
    print(f"  Avg Gas/s: {mdbx['gas_per_second'].mean() / 1e9:.3f} Ggas/s")
    print(f"  Avg Latency: {mdbx['latency_ms'].mean():.2f} ms")
    print(f"  Total Time: {mdbx['latency_ms'].sum() / 1000:.2f} s")
    
    print("\nRocksDB:")
    print(f"  Avg Gas/s: {rocks['gas_per_second'].mean() / 1e9:.3f} Ggas/s")
    print(f"  Avg Latency: {rocks['latency_ms'].mean():.2f} ms")
    print(f"  Total Time: {rocks['latency_ms'].sum() / 1000:.2f} s")
    
    print("\nDelta (RocksDB - MDBX):")
    gas_delta = (rocks['gas_per_second'].mean() - mdbx['gas_per_second'].mean()) / mdbx['gas_per_second'].mean() * 100
    latency_delta = (rocks['latency_ms'].mean() - mdbx['latency_ms'].mean()) / mdbx['latency_ms'].mean() * 100
    print(f"  Gas Throughput: {gas_delta:+.2f}%")
    print(f"  Latency: {latency_delta:+.2f}%")

if __name__ == "__main__":
    import sys
    if len(sys.argv) != 4:
        print("Usage: compare_benchmarks.py <mdbx.csv> <rocks.csv> <output_dir>")
        sys.exit(1)
    compare_benchmarks(sys.argv[1], sys.argv[2], sys.argv[3])
```

## Expected Outcomes

### Primary Metrics

1. **Gas Throughput (Ggas/s)**: Should be similar or slightly better with RocksDB due to batched writes
2. **Block Latency**: May see reduced variance with RocksDB's LSM-tree writes
3. **Stage Time**: `TransactionLookup`, `IndexAccountHistory`, `IndexStorageHistory` stages should show different write patterns

### Secondary Observations

1. **Disk I/O**: RocksDB uses write-ahead logging, may show different I/O patterns
2. **Memory Usage**: RocksDB has configurable block cache, monitor RSS
3. **CPU Usage**: LSM compaction runs in background, watch for CPU spikes

## Jaeger Trace Analysis

Key spans to analyze in Jaeger:

1. `new_payload` - Total newPayload handling time
2. `execute_block` - Block execution time
3. `commit_state` - State commitment including DB writes
4. `tx_lookup_stage` - Transaction lookup indexing
5. `account_history_stage` - Account history indexing
6. `storage_history_stage` - Storage history indexing

Compare span durations between MDBX and RocksDB runs to identify bottlenecks.

## Cleanup

After benchmarking:

```bash
# Stop Jaeger
docker stop jaeger

# Reset Prometheus config (remove benchmark jobs)
cd /home/yk/tempo/reth-grafana/monitoring
# Edit prometheus.yml to remove benchmark jobs
docker-compose restart prometheus

# Clean up benchmark data directories (optional)
rm -rf $HOODI_DATADIR_MDBX $HOODI_DATADIR_ROCKS
```

## Troubleshooting

### RocksDB not enabled
Ensure the `rocksdb` feature is compiled and all three `--storage.*-in-rocksdb` flags are passed.

### Metrics not appearing
Check that `--metrics localhost:<port>` is set and Prometheus is scraping the correct port.

### JWT authentication fails
Ensure the JWT secret file is the same for reth and reth-bench.

### Out of disk space
RocksDB creates separate data in `<datadir>/rocksdb/`. Monitor disk usage.
