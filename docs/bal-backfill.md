# BAL backfill

`reth node --bal.backfill --bal.max-concurrent-requests 4` starts one background task.
It scans canonical headers within `--db.balstore-cache-size` blocks of the head,
skips BALs already in the configured store, and downloads the missing BALs from eth/71 peers.
The default retention distance comes from `BalConfig`.
The default node uses the same setting for BAL pruning; `--prune.*` flags do not change it.
The task also checks the store's pruning policy before requests and inserts, so BALs
pruned by a store that has already seen newer payloads are not downloaded again.

If the latest header has no BAL hash, the task stops there. Otherwise it remembers
the first remaining gap in memory, so retries skip pre-BAL headers and the completed
prefix. Once caught up, an unchanged head needs no header or BAL-store reads. A direct
child whose BAL is already cached needs only one BAL lookup and skips the range scan
and flush. Missing BALs and head jumps still trigger backfill; a reorg resets progress.

The task runs on startup, on canonical chain updates, and every 30 seconds while
idle. Requests contain at most 16 hashes. Short responses continue with the
unreturned suffix; unavailable BALs and request errors are retried on the next
pass. Responses must match the canonical header's BAL hash. Reorged and expired
results are discarded. Requests and inserts are capped at the current canonical head,
including when the head moves backward. Successful inserts are flushed through `BalStore`.
Durability follows the configured store; the default node store is in memory.

Metrics use the `reth_downloaders_bal_` prefix: `downloaded`, `requested`, `skipped`,
`unavailable`, and `invalid` counters, plus an `in_flight_requests` gauge for the
concurrency bound. `requested` counts BAL hashes, including retries. `downloaded`
counts validated inserts, including buffered BALs whose flush must be retried.

Backfill is disabled by default. It fills retained canonical gaps independently
of execution, with no pipeline stages, persistent checkpoints, or execution eligibility policy.

## Testing on Platåberget (glamsterdam-devnet-8)

Use the feature branch's `reth` binary and a devnet-8 consensus client. The
[network page](https://plataberget.dev/) links the
[official configuration](https://github.com/ethpandaops/glamsterdam-devnets/tree/master/network-configs/devnet-8/metadata)
and checkpoint endpoint. EL discovery uses discv5.

Build Reth with `cargo build --release --bin reth`. In a separate data directory,
download `genesis.json`, `config.yaml`, `genesis.ssz`, `bootstrap_nodes.yaml`,
`el_enrs.txt`, `deposit_contract.txt`, and `deposit_contract_block.txt` from the
configuration directory above into `config/`. Generate a separate JWT secret:

```sh
(umask 077; openssl rand -hex 32 > jwt.hex)
```

Choose unused ports. For example, start Reth with:

```sh
reth node --chain config/genesis.json --datadir reth-data \
  --bal.backfill --bal.max-concurrent-requests 2 --db.balstore-cache-size 256 \
  --http --http.addr 127.0.0.1 --http.port 18545 --http.api eth,net,web3,debug \
  --authrpc.addr 127.0.0.1 --authrpc.port 18551 --authrpc.jwtsecret jwt.hex \
  --metrics 127.0.0.1:19001 --ipcdisable \
  --port 31303 --disable-discv4-discovery \
  --discovery.v5.port 19200 --discovery.v5.port.ipv6 19200 \
  --bootnodes "$(paste -sd, config/el_enrs.txt)" \
  --log.stdout.filter info,reth::bal=debug
```

Run `ethpandaops/lighthouse:glamsterdam-devnet-8` with the same configuration and
JWT file mounted. The corresponding Lighthouse command is:

```sh
lighthouse --testnet-dir config bn --datadir lighthouse-data \
  --execution-endpoint http://127.0.0.1:18551 --execution-jwt jwt.hex \
  --checkpoint-sync-url https://checkpoint-sync.plataberget.ethpandaops.io \
  --port 19010 --quic-port 19011 \
  --http --http-address 127.0.0.1 --http-port 15052 \
  --metrics --metrics-address 127.0.0.1 --metrics-port 15054
```

Wait until the EL catches up. Restarting Reth with its default in-memory BAL
store creates historical gaps without changing chain data. With backfill enabled,
`reth::bal` logs should show `Filled BAL gaps`, and
`reth_downloaders_bal_downloaded` should increase in the metrics endpoint. Use
`debug_getRawBlockAccessList` for historical blocks inside the retention window
to check the returned BALs against the corresponding header commitments. This
RPC can also recompute a missing BAL, so use the download counter and gap-fill
logs to establish that backfill supplied the data. A restart without
`--bal.backfill` must produce no backfill requests or gap-fill logs.

Focused tests: `cargo nextest run -p reth-downloaders -p reth-node-core -E 'test(bal::) or test(parse_bal_args)'`.
