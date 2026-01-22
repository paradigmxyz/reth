---
title: Op-reth Base mainnet nodes stops syncing
labels:
    - A-op-reth
    - A-rpc
    - C-bug
assignees:
    - Syzygy106
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.007862Z
info:
    author: jonathanudd
    created_at: 2025-11-20T20:22:18Z
    updated_at: 2025-12-09T14:37:27Z
---

### Describe the bug

Today several of our Base mainnet archive nodes has stopped syncing and we think it might be after some heavy eth_calls where sent to them but we are not sure.
After a restart they start syncing again after they find peers.

### Steps to reproduce

1. Have a running op-reth Base mainnet archive node
2. Send heavy eth_calls
3. Node stops syncing
4. A restart seems to let the node start syncing again

### Node logs

```text
I tried to upload the debug logs in the window above but it failed, maybe the file where to large?
This is the logs from the op-reth service.

Nov 20 19:46:48 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:48.767650Z  INFO State root task finished state_root=0xe5245da004705a29c75e8c09aa7fde286351098fa7a9bebfacd370ca347bbbfc elapsed=5.86µs
Nov 20 19:46:48 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:48.768186Z  INFO Block added to canonical chain number=38439330 hash=0x6df11d7fd2c716d26a35d00d9c74d19af82a299daaa7b91773be74f8cb246031 peers=29 txs=305 gas_used=46.98Mgas gas_throughput=32.80Mgas/second gas_limit=300.00Mgas full=15.7% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=1.432420815s
Nov 20 19:46:48 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:48.864600Z  INFO Canonical chain committed number=38439330 hash=0x6df11d7fd2c716d26a35d00d9c74d19af82a299daaa7b91773be74f8cb246031 elapsed=646.036µs
Nov 20 19:46:49 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:49.330680Z  INFO Received block from consensus engine number=38439331 hash=0x4966692b43fdcb03c55df6b225b6f6b46707705793d940761f2105ca4ab738bd
Nov 20 19:46:50 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:50.058999Z  INFO State root task finished state_root=0x4bab7e92eabf303db64bff11bf23cde32d627ae3f3582325c4e49ff891633fa5 elapsed=4.74µs
Nov 20 19:46:50 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:50.059276Z  INFO Block added to canonical chain number=38439331 hash=0x4966692b43fdcb03c55df6b225b6f6b46707705793d940761f2105ca4ab738bd peers=29 txs=247 gas_used=51.34Mgas gas_throughput=73.75Mgas/second gas_limit=300.00Mgas full=17.1% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=696.141908ms
Nov 20 19:46:50 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:50.088141Z  INFO Canonical chain committed number=38439331 hash=0x4966692b43fdcb03c55df6b225b6f6b46707705793d940761f2105ca4ab738bd elapsed=539.857µs
Nov 20 19:46:51 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:51.070674Z  WARN HTTP request to sequencer failed err=server returned an error response: error code -32000: nonce too low: next nonce 141009, tx nonce 140980
Nov 20 19:46:51 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:51.070978Z  WARN Failed to forward transaction to sequencer err=server returned an error response: error code -32000: nonce too low: next nonce 141009, tx nonce 140980
Nov 20 19:46:51 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:51.376798Z  INFO Received block from consensus engine number=38439332 hash=0x7fc8664f6a050fcf6ad0dc1d76c1c917573a917dcb4b013464fcf6bdcb954c92
Nov 20 19:46:51 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:51.462362Z  WARN Blocked waiting for execution cache mutex blocked_for=31.338838ms
Nov 20 19:46:53 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:53.411791Z  WARN HTTP request to sequencer failed err=server returned an error response: error code -32000: nonce too low: next nonce 141009, tx nonce 140980
Nov 20 19:46:53 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:53.411806Z  WARN Failed to forward transaction to sequencer err=server returned an error response: error code -32000: nonce too low: next nonce 141009, tx nonce 140980
Nov 20 19:46:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:55.027008Z  INFO State root task finished state_root=0xfce71e9e8fe76c02334b0f1b369feb422d2234b91bc2bf61cf883acdcc27d87a elapsed=93.104061ms
Nov 20 19:46:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:55.027440Z  INFO Block added to canonical chain number=38439332 hash=0x7fc8664f6a050fcf6ad0dc1d76c1c917573a917dcb4b013464fcf6bdcb954c92 peers=29 txs=270 gas_used=56.00Mgas gas_throughput=15.45Mgas/second gas_limit=300.00Mgas full=18.7% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=3.625258143s
Nov 20 19:46:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:55.070305Z  INFO Canonical chain committed number=38439332 hash=0x7fc8664f6a050fcf6ad0dc1d76c1c917573a917dcb4b013464fcf6bdcb954c92 elapsed=610.726µs
Nov 20 19:46:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:46:55.116267Z  INFO Received block from consensus engine number=38439333 hash=0xa71eda1782d44f01bc4709c6a6b19cc8b2d2aba0bb50e1d136172080cf4ab8a0
Nov 20 19:47:00 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:47:00.196576Z  INFO State root task finished state_root=0x07177ca4e82e278d74f1e3fe36b0a2c224f48346b092d4d6c43644118bca9662 elapsed=4.15µs
Nov 20 19:47:00 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:47:00.197293Z  INFO Block added to canonical chain number=38439333 hash=0xa71eda1782d44f01bc4709c6a6b19cc8b2d2aba0bb50e1d136172080cf4ab8a0 peers=29 txs=226 gas_used=37.11Mgas gas_throughput=7.34Mgas/second gas_limit=300.00Mgas full=12.4% base_fee=0.00Gwei blobs=0 excess_blobs=0 elapsed=5.055000268s
Nov 20 19:47:00 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:47:00.209042Z  INFO Canonical chain committed number=38439333 hash=0xa71eda1782d44f01bc4709c6a6b19cc8b2d2aba0bb50e1d136172080cf4ab8a0 elapsed=403.567µs
Nov 20 19:47:00 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:47:00.603123Z  INFO Received block from consensus engine number=38439334 hash=0xd4ba51222fdb1c4427265425e1ed0b487efdf48d99b8b5dfa17195fb066f93bd
Nov 20 19:47:10 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:47:10.457418Z  INFO Status connected_peers=29 latest_block=38439333
Nov 20 19:48:25 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:48:25.457689Z  INFO Status connected_peers=29 latest_block=38439333
Nov 20 19:49:40 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:49:40.457359Z  INFO Status connected_peers=29 latest_block=38439333
Nov 20 19:50:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:50:55.457122Z  INFO Status connected_peers=29 latest_block=38439333
Nov 20 19:52:00 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:52:00.614518Z  WARN Long-lived read transaction has been timed out open_duration=300.000068862s backtrace=None
Nov 20 19:52:05 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:52:05.614559Z  WARN Long-lived read transaction has been timed out open_duration=300.000170119s backtrace=None
Nov 20 19:52:05 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:52:05.614862Z  WARN Long-lived read transaction has been timed out open_duration=300.000221939s backtrace=None
Nov 20 19:52:10 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:52:10.457284Z  INFO Status connected_peers=29 latest_block=38439333
Nov 20 19:53:25 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:53:25.457200Z  INFO Status connected_peers=30 latest_block=38439333
Nov 20 19:53:47 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:53:47.432194Z  WARN Beacon client online, but no consensus updates received for a while. This may be because of a reth error, or an error in the beacon client! Please investigate reth and beacon client logs! period=407.234330395s
Nov 20 19:54:40 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:54:40.456656Z  INFO Status connected_peers=30 latest_block=38439333
Nov 20 19:55:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:55:55.456729Z  INFO Status connected_peers=30 latest_block=38439333
Nov 20 19:56:54 juju-83b8d7-0 systemd[1]: Stopping opreth.service - op-reth Client...
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:54.876605Z  INFO Wrote network peers to file peers_file="/home/op/.datadir/known-peers.json"
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:54.903288Z  WARN The database read transaction has been open for too long. Backtrace:
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    0: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    1: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    2: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    3: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    4: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    5: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    6: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    7: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    8: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    9: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   10: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   11: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   12: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   13: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   14: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   15: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   16: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   17: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   18: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   19: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   20: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:  open_duration=594.23454736s self.txn_id=278897888
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:54.931299Z  WARN The database read transaction has been open for too long. Backtrace:
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    0: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    1: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    2: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    3: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    4: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    5: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    6: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    7: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    8: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    9: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   10: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:  open_duration=589.313671961s self.txn_id=278897888
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:54.932894Z  WARN The database read transaction has been open for too long. Backtrace:
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    0: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    1: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    2: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    3: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    4: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    5: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    6: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    7: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    8: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:    9: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:   10: <unknown>
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]:  open_duration=589.318215023s self.txn_id=278897888
Nov 20 19:56:54 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:54.964086Z ERROR Failed to send internal event: SendError { .. }
Nov 20 19:56:55 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:55.764573Z  WARN Blocked waiting for execution cache mutex blocked_for=765.266812ms
Nov 20 19:56:56 juju-83b8d7-0 op-reth[1956338]: 2025-11-20T19:56:56.080541Z  WARN Failed to compute state root in parallel
Nov 20 19:56:59 juju-83b8d7-0 systemd[1]: opreth.service: Deactivated successfully.
Nov 20 19:56:59 juju-83b8d7-0 systemd[1]: Stopped opreth.service - op-reth Client.
Nov 20 19:56:59 juju-83b8d7-0 systemd[1]: ^[[0;1;39m^[[0;1;31m^[[0;1;39mopreth.service: Consumed 1h 25min 50.145s CPU time, 29.3G memory peak, 0B memory swap peak.^[[0m
Nov 20 19:56:59 juju-83b8d7-0 systemd[1]: Started opreth.service - op-reth Client.
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.312441Z  INFO Initialized tracing, debug log directory: /home/op/.cache/reth/logs/base
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.313706Z  INFO Starting reth version="1.9.3 (27a8c0f)"
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.313732Z  INFO Opening database path="/home/op/.datadir/db"
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.325598Z  INFO Launching node
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.328409Z  INFO Configuration loaded path="/home/op/.datadir/reth.toml"
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.338746Z  INFO Verifying storage consistency.
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.341423Z  INFO Database opened
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.341568Z  INFO Starting metrics endpoint at 0.0.0.0:9001
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.341966Z  INFO
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: Pre-merge hard forks (block based):
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Bedrock                          @0
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: Post-merge hard forks (timestamp based):
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Regolith                         @0
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Canyon                           @1704992401
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Ecotone                          @1710374401
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Fjord                            @1720627201
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Granite                          @1726070401
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Holocene                         @1736445601
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Isthmus                          @1746806401
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: - Jovian                           @1764691201
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.378827Z  INFO Transaction pool initialized
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.379286Z  INFO Loading saved peers file=/home/op/.datadir/known-peers.json
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.644533Z  INFO P2P networking initialized enode=enode://e0d09017142ccff5333a7952c9809c4ab43a58b40fe9b4b4be67f229488ddeb9a20dafd18e990862e4ebab8571b61663536a6a8a4af62f7c555cb93e542c07cd@127.0.0.1:30393
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.644850Z  INFO StaticFileProducer initialized
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.645334Z  INFO Pruner initialized prune_config=PruneConfig { block_interval: 5, segments: PruneModes { sender_recovery: None, transaction_lookup: None, receipts: None, account_history: None, storage_history: None, bodies_history: None, merkle_changesets: Distance(10064), receipts_log_filter: ReceiptsLogPruneConfig({}) } }
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.645878Z  INFO Consensus engine initialized
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.655913Z  INFO Engine API handler initialized
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.666800Z  INFO RPC auth server started url=127.0.0.1:8551
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.667193Z  INFO RPC IPC server started path=/tmp/reth.ipc
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.667207Z  INFO RPC HTTP server started url=0.0.0.0:8545
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.667211Z  INFO RPC WS server started url=0.0.0.0:8546
Nov 20 19:56:59 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:56:59.667264Z  INFO Starting consensus engine
Nov 20 19:57:02 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:57:02.649636Z  INFO Status connected_peers=0 latest_block=38439331
Nov 20 19:57:07 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:57:07.355242Z  INFO Received block from consensus engine number=38439640 hash=0x4ffc7fdd5effebf0c277564dbe1c4508fb833982dc710225c2ff1ea7707aa300
Nov 20 19:57:07 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:57:07.360550Z  INFO Received forkchoice updated message when syncing head_block_hash=0x4ffc7fdd5effebf0c277564dbe1c4508fb833982dc710225c2ff1ea7707aa300 safe_block_hash=0x0000000000000000000000000000000000000000000000000000000000000000 finalized_block_hash=0x0000000000000000000000000000000000000000000000000000000000000000
Nov 20 19:57:09 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:57:09.495366Z  INFO Received block from consensus engine number=38439641 hash=0xc467ae16308d59029298db7c6f36f9cd05e215a0e68d00fd7e942fcd9a2c6cb4
Nov 20 19:57:11 juju-83b8d7-0 op-reth[1960127]: 2025-11-20T19:57:11.399125Z  INFO Received block from consensus engine number=38439642 hash=0xbd086b4e9c436bc8db249887d30fd9ae711f06c4eb81e9e3ff087529f96b87b8
```

### Platform(s)

Linux (x86)

### Container Type

LXC/LXD

### What version/commit are you on?

1.9.3

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

Base mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
