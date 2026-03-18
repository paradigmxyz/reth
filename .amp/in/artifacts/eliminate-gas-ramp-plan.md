# Plan: Eliminate Gas Ramp Step from Big Block Benchmarks

## Background

The current big block benchmark pipeline requires ~6,800 empty blocks to slowly ramp the gas limit from 30M to 1G (constrained by the 1/1024 consensus rule). Two new reth CLI flags eliminate this:

- `--testing.skip-gas-limit-ramp-check`: Disables the 1/1024 gas limit bound divisor consensus check.
- `--testing.gas-limit`: Overrides the gas limit used by `testing_buildBlockV1`.
- `--rpc.max-request-size <MB>`: Increase the HTTP RPC body size limit (default 15 MB). Required when `testing_buildBlockV1` sends large transaction lists for 1G+ gas blocks — at ~24K txs the request body exceeds 15 MB.

These flags are already committed in the current branch.

---

## Repo 1: `paradigmxyz/reth`

### A. `.github/scripts/bench-reth-run.sh`

1. In big blocks mode, add `--testing.skip-gas-limit-ramp-check --testing.gas-limit 1000000000` to `RETH_ARGS`.
2. Remove `--gas-ramp-dir "$BIG_BLOCKS_DIR/gas-ramp-dir"` from the `reth-bench replay-payloads` invocation.
3. Remove the gas ramp block counting logic (`GAS_RAMP_COUNT`, `gas_ramp_blocks.txt`).

### B. `.github/workflows/bench.yml`

1. In "Download big blocks" step (~L841): remove the `gas-ramp-dir` existence check. Only verify `payloads/` exists.
2. In "Compare" step (~L1052-1058): remove the `--gas-ramp-blocks` summary arg logic.

### C. `.github/scripts/bench-reth-summary.py` & `bench-slack-notify.js`

1. Remove `--gas-ramp-blocks` CLI argument and its usage from the summary script (~L475, L557).
2. Remove gas ramp reporting from the Slack notification (~L126-127 in `bench-slack-notify.js`).

### D. `bin/reth-bench/src/bench/replay_payloads.rs`

1. Keep `--gas-ramp-dir` for backward compatibility but make it optional/no-op with a deprecation warning, or remove it outright.

### E. (Optional cleanup) `bin/reth-bench/src/bench/gas_limit_ramp.rs`

1. Deprecate or remove the `gas-limit-ramp` subcommand entirely — it's no longer needed.

---

## Repo 2: `tempoxyz/helm-charts`

### `big-block-generator-run.nu` (nightly CronWorkflow script)

1. Start reth with `--testing.skip-gas-limit-ramp-check --testing.gas-limit 1G` flags added to the node args.
2. **Remove** the `reth-bench gas-limit-ramp` step entirely.
3. **Remove** archiving of `gas-ramp-dir/` — the archive only needs `payloads/`.
4. The `generate-big-block` step stays the same (it uses `testing_buildBlockV1` which now respects the overridden gas limit).

---

## Testing on a Dev Box

```bash
# 1. Build reth and reth-bench from your branch
cargo build --profile profiling -p reth -p reth-bench

# 2. Start reth with an existing snapshot/datadir, enabling the new flags
./target/profiling/reth node \
  --datadir /path/to/datadir \
  --http --http.api eth,net,web3,reth,testing \
  --rpc.max-request-size 500 \
  --testing.skip-invalid-transactions \
  --testing.skip-gas-limit-ramp-check \
  --testing.gas-limit 1000000000

# 3. Generate big blocks (proves gas limit jump works end-to-end)
./target/profiling/reth-bench generate-big-block \
  --rpc-url https://some-archive-node \
  --engine-rpc-url http://localhost:8551 \
  --jwt-secret /path/to/datadir/jwt.hex \
  --target-gas 1G \
  --from-block <tip-block-number> \
  --count 2 --execute \
  --output-dir /tmp/big-block-test

# 4. Replay without gas ramp (proves replay works without --gas-ramp-dir)
./target/profiling/reth-bench replay-payloads \
  --engine-rpc-url http://localhost:8551 \
  --jwt-secret /path/to/datadir/jwt.hex \
  --payload-dir /tmp/big-block-test \
  --reth-new-payload \
  --output /tmp/big-block-replay

# 5. Verify: check the block gas_limit in the output payload JSON
jq '.executionPayload.gasLimit' /tmp/big-block-test/payload_block_*.json
# Should show "0x3b9aca00" (1G in hex) or similar large value
```

### Unwinding after generate-big-block

`generate-big-block --execute` advances the canonical chain. Before replaying, stop reth and unwind back to the original tip:

```bash
# Record the tip block number BEFORE running generate-big-block
TIP=$(cast rpc --rpc-url http://localhost:8545 eth_blockNumber | jq -r '.' | printf '%d' $(cat))

# ... run generate-big-block --count 2 --execute ...

# Stop reth, then unwind to the original tip
kill $RETH_PID
./target/profiling/reth stage unwind to-block $TIP --datadir /path/to/datadir

# Restart reth, then replay
./target/profiling/reth node ... &
# wait for ready...
reth-bench replay-payloads --payload-dir /tmp/big-block-test ...
```

### What to verify

- Block gas limit jumps directly to 1G (no 6800-block ramp needed)
- `generate-big-block` produces blocks that fill close to 1G gas
- `replay-payloads` succeeds without `--gas-ramp-dir`
- Consensus validation passes (the skip flag works)
