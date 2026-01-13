# RocksDB vs MDBX Benchmark Plan

## Objective
Compare performance of history index operations between MDBX and RocksDB storage backends using `reth-bench new-payload-fcu` for 200 blocks.

## Current State
- Hoodi snapshot at block 2,020,000 (MDBX only, no RocksDB data)
- Two directories: `hoodi-bench-mdbx` and `hoodi-bench-rocks`
- `reth db settings set` does NOT support RocksDB flags (needs code change)

## Prerequisites

### 1. Add RocksDB Settings Commands
The current `reth db settings set` only supports:
- `receipts`
- `transaction_senders`
- `account_changesets`

**Need to add:**
- `storages_history` - Enable/disable StoragesHistory in RocksDB
- `account_history` - Enable/disable AccountsHistory in RocksDB
- `tx_hash_numbers` - Enable/disable TransactionHashNumbers in RocksDB

**File to modify:** `crates/cli/commands/src/db/settings.rs`

---

## Benchmark Phases

### Phase 1: MDBX Baseline Benchmark

#### 1.1 Setup
```bash
# Kill any running nodes
pkill -f "reth node"

# Copy fresh snapshot for MDBX test
rm -rf ~/.local/share/reth/hoodi-bench-mdbx
cp -r ~/.local/share/reth/hoodi-snapshot ~/.local/share/reth/hoodi-bench-mdbx
```

#### 1.2 Unwind 200 Blocks
```bash
./target/release/reth db unwind \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx \
  --target 2019800
```

#### 1.3 Verify Storage Settings
```bash
./target/release/reth db settings get \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx
```
Expected: All RocksDB flags = false (MDBX only)

#### 1.4 Start Source Node (Block Provider)
```bash
./target/release/reth node \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-rocks \
  --http \
  --http.port 8547 \
  --authrpc.port 8552 \
  --discovery.port 30304 \
  --port 30304
```

#### 1.5 Start Target Node (MDBX Benchmark)
```bash
./target/release/reth node \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx \
  --http \
  --http.port 8545 \
  --authrpc.port 8551 \
  --authrpc.jwtsecret ~/.local/share/reth/hoodi-bench-mdbx/jwt.hex \
  --metrics 127.0.0.1:9100
```

#### 1.6 Run Benchmark
```bash
./target/release/reth-bench new-payload-fcu \
  --rpc-url http://127.0.0.1:8547 \
  --engine-rpc-url http://127.0.0.1:8551 \
  --jwtsecret ~/.local/share/reth/hoodi-bench-mdbx/jwt.hex \
  --from 2019801 \
  --to 2020000 \
  --output mdbx_benchmark_results.json
```

#### 1.7 Collect Metrics
```bash
curl -s http://127.0.0.1:9100/metrics > mdbx_metrics.txt
```

---

### Phase 2: Enable RocksDB and Benchmark

#### 2.1 Stop Nodes
```bash
pkill -f "reth node"
```

#### 2.2 Reset and Unwind
```bash
# Reset to clean snapshot
rm -rf ~/.local/share/reth/hoodi-bench-mdbx
cp -r ~/.local/share/reth/hoodi-snapshot ~/.local/share/reth/hoodi-bench-mdbx

# Unwind to same block
./target/release/reth db unwind \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx \
  --target 2019800
```

#### 2.3 Enable RocksDB Storage Settings
```bash
# After code changes are made:
./target/release/reth db settings set storages_history true \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx

./target/release/reth db settings set account_history true \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx

./target/release/reth db settings set tx_hash_numbers true \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx
```

#### 2.4 Verify Settings Changed
```bash
./target/release/reth db settings get \
  --chain hoodi \
  --datadir ~/.local/share/reth/hoodi-bench-mdbx
```
Expected: All RocksDB flags = true

#### 2.5 Start Nodes and Benchmark
Same as Phase 1 steps 1.4-1.7, but save results as `rocksdb_benchmark_results.json` and `rocksdb_metrics.txt`

---

## Code Changes Required

### File: `crates/cli/commands/src/db/settings.rs`

Add to `SetCommand` enum:
```rust
/// Store storages history in RocksDB instead of MDBX
StoragesHistory {
    #[clap(action(ArgAction::Set))]
    value: bool,
},
/// Store account history in RocksDB instead of MDBX
AccountHistory {
    #[clap(action(ArgAction::Set))]
    value: bool,
},
/// Store transaction hash numbers in RocksDB instead of MDBX
TxHashNumbers {
    #[clap(action(ArgAction::Set))]
    value: bool,
},
```

Add to `set()` function match:
```rust
SetCommand::StoragesHistory { value } => {
    if settings.storages_history_in_rocksdb == value {
        println!("storages_history_in_rocksdb is already set to {}", value);
        return Ok(());
    }
    settings.storages_history_in_rocksdb = value;
    println!("Set storages_history_in_rocksdb = {}", value);
}
SetCommand::AccountHistory { value } => {
    if settings.account_history_in_rocksdb == value {
        println!("account_history_in_rocksdb is already set to {}", value);
        return Ok(());
    }
    settings.account_history_in_rocksdb = value;
    println!("Set account_history_in_rocksdb = {}", value);
}
SetCommand::TxHashNumbers { value } => {
    if settings.transaction_hash_numbers_in_rocksdb == value {
        println!("transaction_hash_numbers_in_rocksdb is already set to {}", value);
        return Ok(());
    }
    settings.transaction_hash_numbers_in_rocksdb = value;
    println!("Set transaction_hash_numbers_in_rocksdb = {}", value);
}
```

---

## Metrics to Compare

### Performance Metrics
- Block execution time (ms/block)
- State root computation time
- Total sync time for 200 blocks
- Throughput (blocks/second)

### Resource Metrics
- Memory usage (RSS)
- Disk I/O (reads/writes)
- CPU utilization
- Database file sizes

### From `reth-bench` Output
- `new_payload` latency (p50, p90, p99)
- `forkchoice_updated` latency (p50, p90, p99)
- Total benchmark duration

---

## Expected Results Analysis

Create comparison table:
| Metric | MDBX | RocksDB | Difference |
|--------|------|---------|------------|
| Avg block time | | | |
| p99 latency | | | |
| Memory peak | | | |
| Disk writes | | | |

---

## Troubleshooting

### If RocksDB directory is empty after enabling
The unwind + re-execute cycle should populate RocksDB tables. If still empty:
1. Check that `Storage settings` shows RocksDB flags as `true`
2. Verify the stages write to RocksDB when `storage_settings.storages_history_in_rocksdb` is true

### Port conflicts
Use different ports for source vs target nodes:
- Source: 8547, 8552, 30304
- Target: 8545, 8551, 30303

---

## Automation Script

A complete automation script will be created at:
`scripts/benchmark_rocksdb_vs_mdbx.sh`
