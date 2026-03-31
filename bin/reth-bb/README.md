# reth-bb

A modified reth node for benchmarking **big block** execution — payloads that merge transactions from multiple consecutive blocks into a single block to simulate high-gas workloads.

> **Not for production use.** reth-bb disables some consensus-related validations to allow artificially large blocks. It is intended solely for performance benchmarking.

## How it works

reth-bb extends the standard Ethereum node with:

1. **Multi-segment execution** — a custom `reth_newPayload` handler that accepts optional `BigBlockData` alongside the payload. When present, the block is executed in multiple segments, each with its own EVM environment (matching the original blocks that were merged).

2. **Relaxed consensus** — the gas-limit bound-divisor check and blob gas validation are skipped, since merged blocks exceed single-block limits.

## Quick start

The full workflow has four steps: **build** binaries, **generate** big blocks,  **start** reth-bb, and **replay** the payloads.

### Prerequisites

- A synced reth datadir for the target chain (e.g. hoodi)
- Rust toolchain

### 1. Build

```bash
cargo build --profile profiling -p reth-bb -p reth-bench
```

### 2. Generate big blocks

Fetch consecutive blocks from an RPC and merge them until a target gas is reached. Use `--from-block` set to the block number following the one the node is currently synced to (i.e. the next block the node would process):

```bash
reth-bench generate-big-block \
    --rpc-url https://rpc.hoodi.ethpandaops.io \
    --chain hoodi \
    --from-block 910020 \
    --target-gas 2G \
    --num-big-blocks 5 \
    --output-dir /tmp/payloads
```

This produces one JSON file per big block in the output directory.

### 3. Start reth-bb

```bash
reth-bb node \
    --datadir /data/reth/hoodi \
    --chain hoodi \
    --http --http.api debug,eth \
    --authrpc.jwtsecret /tmp/jwt.hex \
    -d
```

### 4. Replay payloads

```bash
reth-bench replay-payloads \
    --engine-rpc-url http://localhost:8551 \
    --jwt-secret /tmp/jwt.hex \
    --payload-dir /tmp/payloads \
    --reth-new-payload
```

The `--reth-new-payload` flag is required for big blocks — it uses the `reth_newPayload` endpoint which carries the multi-segment execution metadata.
