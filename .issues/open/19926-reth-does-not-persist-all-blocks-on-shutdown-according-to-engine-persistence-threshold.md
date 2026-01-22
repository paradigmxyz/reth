---
title: Reth does not persist all blocks on shutdown, according to engine.persistence-threshold
labels:
    - A-op-reth
    - A-pruning
    - C-bug
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.008573Z
info:
    author: kahuang
    created_at: 2025-11-24T00:50:46Z
    updated_at: 2026-01-17T02:15:56Z
---

### Describe the bug

2025-11-23 22:15:27,128 CRIT Server 'unix_http_server' running without any HTTP authentication checking
2025-11-23T22:15:28.171191Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/57285
2025-11-23T22:15:28.171940Z  INFO Starting reth version="1.9.1-dev (595879c)"
2025-11-23T22:15:28.171958Z  INFO Opening database path="/db/db"
2025-11-23T22:15:28.423838Z  INFO Launching node
2025-11-23T22:15:28.428563Z  INFO Configuration loaded path="/db/reth.toml"
2025-11-23T22:15:28.485190Z  INFO Verifying storage consistency.
2025-11-23T22:15:28.531141Z  INFO Database opened
2025-11-23T22:15:28.531190Z  INFO Starting metrics endpoint at 0.0.0.0:6060
2025-11-23T22:15:28.534029Z  INFO
Pre-merge hard forks (block based):
- Bedrock                          @0
Post-merge hard forks (timestamp based):
- Regolith                         @0
- Canyon                           @0
- Ecotone                          @0
- Fjord                            @0
- Granite                          @0
- Holocene                         @0
- Isthmus                          @0
2025-11-23T22:15:28.637888Z  INFO Transaction pool initialized
2025-11-23T22:15:28.640664Z  INFO Loading saved peers file=/db/known-peers.json
2025-11-23T22:15:29.660772Z  INFO P2P networking initialized enode=enode://ae1c9ad15258af78e02fc901797ae449ba3fecedfb255d928acc1343e265d909300853d27b6504670eef51a6baa90e9f875ef4b0560bb73d8c7c6669cdf87959@35.227.137.72:30303
2025-11-23T22:15:29.660906Z  INFO StaticFileProducer initialized
2025-11-23T22:15:29.661896Z  INFO Pruner initialized prune_config=PruneConfig { block_interval: 5, segments: PruneModes { sender_recovery: Some(Distance(100000)), transaction_lookup: Some(Full), receipts: Some(Distance(100000)), account_history: Some(Distance(100000)), storage_history: Some(Distance(100000)), bodies_history: None, merkle_changesets: Distance(10064), receipts_log_filter: () } }
2025-11-23T22:15:29.663816Z  INFO Consensus engine initialized
2025-11-23T22:15:29.663918Z  INFO Engine API handler initialized
2025-11-23T22:15:29.665606Z  INFO RPC auth server started url=127.0.0.1:8551
2025-11-23T22:15:29.665849Z  INFO RPC IPC server started path=/tmp/reth.ipc
2025-11-23T22:15:29.665858Z  INFO RPC HTTP server started url=0.0.0.0:80
2025-11-23T22:15:29.665861Z  INFO RPC WS server started url=0.0.0.0:80
2025-11-23T22:15:29.665927Z  INFO Starting consensus engine
2025-11-23T22:15:31.073659Z  INFO Forkchoice updated head_block_hash=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1 safe_block_hash=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1 finalized_block_hash=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:15:32.665378Z  INFO Status connected_peers=0 latest_block=873297
2025-11-23T22:15:43.352483Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:15:58.374022Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:16:11.131028Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:16:27.192707Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:16:36.902033Z  INFO Block added to canonical chain number=873298 hash=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023 peers=2 txs=1001 gas_used=549.31Mgas gas_throughput=31660.35Ggas/second gas_limit=10.00Ggas full=5.5% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=17.35µs
2025-11-23T22:16:36.911289Z  INFO Received block from consensus engine number=873298 hash=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023
2025-11-23T22:16:36.923898Z  INFO Canonical chain committed number=873298 hash=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023 elapsed=3.863753ms
2025-11-23T22:16:36.959368Z  INFO New payload job created id=0x03ddb9ab1543c45f parent=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023
2025-11-23T22:16:40.865792Z  INFO Block added to canonical chain number=873299 hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 peers=2 txs=104 gas_used=9.98Ggas gas_throughput=1151191.32Ggas/second gas_limit=10.00Ggas full=99.8% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=8.67µs
2025-11-23T22:16:40.867032Z  INFO Received block from consensus engine number=873299 hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143
2025-11-23T22:16:40.869131Z  INFO Canonical chain committed number=873299 hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 elapsed=96.062µs
2025-11-23T22:16:47.665149Z  INFO Status connected_peers=2 latest_block=873299
2025-11-23T22:18:02.668078Z  INFO Status connected_peers=2 latest_block=873299
2025-11-23T22:18:28.304731Z  INFO Wrote network peers to file peers_file="/db/known-peers.json"

As you can see here, latest_block is 873299. We then stop the replica...

replica: stopped

2025-11-23T22:24:29.415544Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/57285
2025-11-23T22:24:29.416313Z  INFO Starting reth version="1.9.1-dev (595879c)"
2025-11-23T22:24:29.416330Z  INFO Opening database path="/db/db"
2025-11-23T22:24:29.422093Z  INFO Launching node
2025-11-23T22:24:29.423777Z  INFO Configuration loaded path="/db/reth.toml"
2025-11-23T22:24:29.424653Z  INFO Verifying storage consistency.
2025-11-23T22:24:29.434455Z  INFO Database opened
2025-11-23T22:24:29.434498Z  INFO Starting metrics endpoint at 0.0.0.0:6060
2025-11-23T22:24:29.434627Z  INFO
Pre-merge hard forks (block based):
- Bedrock                          @0
Post-merge hard forks (timestamp based):
- Regolith                         @0
- Canyon                           @0
- Ecotone                          @0
- Fjord                            @0
- Granite                          @0
- Holocene                         @0
- Isthmus                          @0
2025-11-23T22:24:29.527852Z  INFO Transaction pool initialized
2025-11-23T22:24:29.529133Z  INFO Loading saved peers file=/db/known-peers.json
replica: started
2025-11-23T22:24:30.552502Z  INFO P2P networking initialized enode=enode://ae1c9ad15258af78e02fc901797ae449ba3fecedfb255d928acc1343e265d909300853d27b6504670eef51a6baa90e9f875ef4b0560bb73d8c7c6669cdf87959@35.227.137.72:30303
2025-11-23T22:24:30.552634Z  INFO StaticFileProducer initialized
2025-11-23T22:24:30.553081Z  INFO Pruner initialized prune_config=PruneConfig { block_interval: 5, segments: PruneModes { sender_recovery: Some(Distance(100000)), transaction_lookup: Some(Full), receipts: Some(Distance(100000)), account_history: Some(Distance(100000)), storage_history: Some(Distance(100000)), bodies_history: None, merkle_changesets: Distance(10064), receipts_log_filter: () } }
2025-11-23T22:24:30.553455Z  INFO Consensus engine initialized
2025-11-23T22:24:30.553561Z  INFO Engine API handler initialized
2025-11-23T22:24:30.555367Z  INFO RPC auth server started url=127.0.0.1:8551
2025-11-23T22:24:30.555660Z  INFO RPC IPC server started path=/tmp/reth.ipc
2025-11-23T22:24:30.555671Z  INFO RPC HTTP server started url=0.0.0.0:80
2025-11-23T22:24:30.555673Z  INFO RPC WS server started url=0.0.0.0:80
2025-11-23T22:24:30.555749Z  INFO Starting consensus engine
2025-11-23T22:24:31.257129Z  INFO Received forkchoice updated message when syncing head_block_hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 safe_block_hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 finalized_block_hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143
2025-11-23T22:24:33.555147Z  INFO Status connected_peers=0 latest_block=873297

Upon starting the latest block is reset to 873297


This is fixed by setting --engine.persistence-threshold=0, but i'd expect reth to persist *all* blocks on shutdown regardless of this param.

### Steps to reproduce

Start a node, advance the blocks. Stop the node. Start it again.

### Node logs

```text
2025-11-23 22:15:27,128 CRIT Server 'unix_http_server' running without any HTTP authentication checking
2025-11-23T22:15:28.171191Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/57285
2025-11-23T22:15:28.171940Z  INFO Starting reth version="1.9.1-dev (595879c)"
2025-11-23T22:15:28.171958Z  INFO Opening database path="/db/db"
2025-11-23T22:15:28.423838Z  INFO Launching node
2025-11-23T22:15:28.428563Z  INFO Configuration loaded path="/db/reth.toml"
2025-11-23T22:15:28.485190Z  INFO Verifying storage consistency.
2025-11-23T22:15:28.531141Z  INFO Database opened
2025-11-23T22:15:28.531190Z  INFO Starting metrics endpoint at 0.0.0.0:6060
2025-11-23T22:15:28.534029Z  INFO
Pre-merge hard forks (block based):
- Bedrock                          @0
Post-merge hard forks (timestamp based):
- Regolith                         @0
- Canyon                           @0
- Ecotone                          @0
- Fjord                            @0
- Granite                          @0
- Holocene                         @0
- Isthmus                          @0
2025-11-23T22:15:28.637888Z  INFO Transaction pool initialized
2025-11-23T22:15:28.640664Z  INFO Loading saved peers file=/db/known-peers.json
2025-11-23T22:15:29.660772Z  INFO P2P networking initialized enode=enode://ae1c9ad15258af78e02fc901797ae449ba3fecedfb255d928acc1343e265d909300853d27b6504670eef51a6baa90e9f875ef4b0560bb73d8c7c6669cdf87959@35.227.137.72:30303
2025-11-23T22:15:29.660906Z  INFO StaticFileProducer initialized
2025-11-23T22:15:29.661896Z  INFO Pruner initialized prune_config=PruneConfig { block_interval: 5, segments: PruneModes { sender_recovery: Some(Distance(100000)), transaction_lookup: Some(Full), receipts: Some(Distance(100000)), account_history: Some(Distance(100000)), storage_history: Some(Distance(100000)), bodies_history: None, merkle_changesets: Distance(10064), receipts_log_filter: () } }
2025-11-23T22:15:29.663816Z  INFO Consensus engine initialized
2025-11-23T22:15:29.663918Z  INFO Engine API handler initialized
2025-11-23T22:15:29.665606Z  INFO RPC auth server started url=127.0.0.1:8551
2025-11-23T22:15:29.665849Z  INFO RPC IPC server started path=/tmp/reth.ipc
2025-11-23T22:15:29.665858Z  INFO RPC HTTP server started url=0.0.0.0:80
2025-11-23T22:15:29.665861Z  INFO RPC WS server started url=0.0.0.0:80
2025-11-23T22:15:29.665927Z  INFO Starting consensus engine
2025-11-23T22:15:31.073659Z  INFO Forkchoice updated head_block_hash=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1 safe_block_hash=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1 finalized_block_hash=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:15:32.665378Z  INFO Status connected_peers=0 latest_block=873297
2025-11-23T22:15:43.352483Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:15:58.374022Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:16:11.131028Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:16:27.192707Z  INFO New payload job created id=0x0312638f65a897f7 parent=0x56e9378f754af0ce8ff7ad7e7341b33dc056826350511ea2c5b01d2a26399be1
2025-11-23T22:16:36.902033Z  INFO Block added to canonical chain number=873298 hash=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023 peers=2 txs=1001 gas_used=549.31Mgas gas_throughput=31660.35Ggas/second gas_limit=10.00Ggas full=5.5% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=17.35µs
2025-11-23T22:16:36.911289Z  INFO Received block from consensus engine number=873298 hash=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023
2025-11-23T22:16:36.923898Z  INFO Canonical chain committed number=873298 hash=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023 elapsed=3.863753ms
2025-11-23T22:16:36.959368Z  INFO New payload job created id=0x03ddb9ab1543c45f parent=0x0a0b56ff56917a1c06c0ace9cb25d1729ba61579d6a6ff40e8c38e9ba21d8023
2025-11-23T22:16:40.865792Z  INFO Block added to canonical chain number=873299 hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 peers=2 txs=104 gas_used=9.98Ggas gas_throughput=1151191.32Ggas/second gas_limit=10.00Ggas full=99.8% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=8.67µs
2025-11-23T22:16:40.867032Z  INFO Received block from consensus engine number=873299 hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143
2025-11-23T22:16:40.869131Z  INFO Canonical chain committed number=873299 hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 elapsed=96.062µs
2025-11-23T22:16:47.665149Z  INFO Status connected_peers=2 latest_block=873299
2025-11-23T22:18:02.668078Z  INFO Status connected_peers=2 latest_block=873299
2025-11-23T22:18:28.304731Z  INFO Wrote network peers to file peers_file="/db/known-peers.json"

As you can see here, latest_block is 873299. We then stop the replica...

replica: stopped

2025-11-23T22:24:29.415544Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/57285
2025-11-23T22:24:29.416313Z  INFO Starting reth version="1.9.1-dev (595879c)"
2025-11-23T22:24:29.416330Z  INFO Opening database path="/db/db"
2025-11-23T22:24:29.422093Z  INFO Launching node
2025-11-23T22:24:29.423777Z  INFO Configuration loaded path="/db/reth.toml"
2025-11-23T22:24:29.424653Z  INFO Verifying storage consistency.
2025-11-23T22:24:29.434455Z  INFO Database opened
2025-11-23T22:24:29.434498Z  INFO Starting metrics endpoint at 0.0.0.0:6060
2025-11-23T22:24:29.434627Z  INFO
Pre-merge hard forks (block based):
- Bedrock                          @0
Post-merge hard forks (timestamp based):
- Regolith                         @0
- Canyon                           @0
- Ecotone                          @0
- Fjord                            @0
- Granite                          @0
- Holocene                         @0
- Isthmus                          @0
2025-11-23T22:24:29.527852Z  INFO Transaction pool initialized
2025-11-23T22:24:29.529133Z  INFO Loading saved peers file=/db/known-peers.json
replica: started
2025-11-23T22:24:30.552502Z  INFO P2P networking initialized enode=enode://ae1c9ad15258af78e02fc901797ae449ba3fecedfb255d928acc1343e265d909300853d27b6504670eef51a6baa90e9f875ef4b0560bb73d8c7c6669cdf87959@35.227.137.72:30303
2025-11-23T22:24:30.552634Z  INFO StaticFileProducer initialized
2025-11-23T22:24:30.553081Z  INFO Pruner initialized prune_config=PruneConfig { block_interval: 5, segments: PruneModes { sender_recovery: Some(Distance(100000)), transaction_lookup: Some(Full), receipts: Some(Distance(100000)), account_history: Some(Distance(100000)), storage_history: Some(Distance(100000)), bodies_history: None, merkle_changesets: Distance(10064), receipts_log_filter: () } }
2025-11-23T22:24:30.553455Z  INFO Consensus engine initialized
2025-11-23T22:24:30.553561Z  INFO Engine API handler initialized
2025-11-23T22:24:30.555367Z  INFO RPC auth server started url=127.0.0.1:8551
2025-11-23T22:24:30.555660Z  INFO RPC IPC server started path=/tmp/reth.ipc
2025-11-23T22:24:30.555671Z  INFO RPC HTTP server started url=0.0.0.0:80
2025-11-23T22:24:30.555673Z  INFO RPC WS server started url=0.0.0.0:80
2025-11-23T22:24:30.555749Z  INFO Starting consensus engine
2025-11-23T22:24:31.257129Z  INFO Received forkchoice updated message when syncing head_block_hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 safe_block_hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143 finalized_block_hash=0x4e76e47ab86e5c52eb4164c56d32e422b50d1bc810bb6ac1f9aa383403317143
2025-11-23T22:24:33.555147Z  INFO Status connected_peers=0 latest_block=873297
```

### Platform(s)

Linux (x86)

### Container Type

Kubernetes

### What version/commit are you on?

1.9.1

### What database version are you on?

1.9.1

### Which chain / network are you on?

op stack chain

### What type of node are you running?

Pruned with custom reth.toml config

### What prune config do you use, if any?

full node config, except logs are also pruned

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
