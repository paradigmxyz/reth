# Pruning & Full Node

> Pruning and full node are new features of Reth,
> and we will be happy to hear about your experience using them either
> on [GitHub](https://github.com/paradigmxyz/reth/issues) or in the [Telegram group](https://t.me/paradigm_reth).  

By default, Reth runs as an archive node. Such nodes have all historical blocks and the state at each of these blocks
available for querying and tracing.

Reth also supports pruning of historical data and running as a full node. This chapter will walk through
the steps for running Reth as a full node, what caveats to expect and how to configure your own pruned node.

## Basic concepts

- Archive node – Reth node that has all historical data from genesis.
- Pruned node – Reth node that has its historical data pruned partially or fully through
a [custom configuration](./config.md#the-prune-section).
- Full Node – Reth node that has the latest state and historical data for only the last 10064 blocks available
for querying in the same way as an archive node.

The node type that was chosen when first [running a node](./run-a-node.md) **can not** be changed after
the initial sync. Turning Archive into Pruned, or Pruned into Full is not supported.

## Modes
### Archive Node

Default mode, follow the steps from the previous chapter on [how to run on mainnet or official testnets](./mainnet.md).

### Pruned Node

To run Reth as a pruned node configured through a [custom configuration](./config.md#the-prune-section),
modify the `reth.toml` file and run Reth in the same way as archive node by following the steps from
the previous chapter on [how to run on mainnet or official testnets](./mainnet.md).

### Full Node

To run Reth as a full node, follow the steps from the previous chapter on
[how to run on mainnet or official testnets](./mainnet.md), and add a `--full` flag. For example:
```bash
RUST_LOG=info reth node \
    --full \
    --authrpc.jwtsecret /path/to/secret \
    --authrpc.addr 127.0.0.1 \
    --authrpc.port 8551
```

## Size

All numbers are as of October 2023 at block number 18.3M for mainnet.

### Archive Node

Archive node occupies at least 2.14TB.

You can track the growth of Reth archive node size with our
[public Grafana dashboard](https://reth.paradigm.xyz/d/2k8BXz24k/reth?orgId=1&refresh=30s&viewPanel=52).

### Pruned Node

Different segments take up different amounts of disk space.
If pruned fully, this is the total freed space you'll get, per segment:

| Segment            | Size  |
|--------------------|-------|
| Sender Recovery    | 75GB  |
| Transaction Lookup | 150GB |
| Receipts           | 250GB |
| Account History    | 240GB |
| Storage History    | 700GB |

### Full Node

Full node occupies at least 950GB.

Essentially, the full node is the same as following configuration for the pruned node:
```toml
[prune]
block_interval = 5

[prune.parts]
sender_recovery = { distance = 10_064 }
# transaction_lookup is not pruned
receipts = { before = 11052984 } # Beacon Deposit Contract deployment block: https://etherscan.io/tx/0xe75fb554e433e03763a1560646ee22dcb74e5274b34c5ad644e7c0f619a7e1d0
account_history = { distance = 10_064 }
storage_history = { distance = 10_064 }

[prune.parts.receipts_log_filter]
# Prune all receipts, leaving only those which contain logs from address `0x00000000219ab540356cbb839cbe05303d7705fa`,
# starting from the block 11052984. This leaves receipts with the logs from the Beacon Deposit Contract.
"0x00000000219ab540356cbb839cbe05303d7705fa" = { before = 11052984 }
```

Meaning, it prunes:
- Account History and Storage History up to the last 10064 blocks
- Sender Recovery up to the last 10064 blocks. The caveat is that it's pruned gradually after the initial sync
is completed, so the disk space is reclaimed slowly.
- Receipts up to the last 10064 blocks, preserving all receipts with the logs from Beacon Deposit Contract

Given the aforementioned segment sizes, we get the following full node size:
```text
Archive Node - Receipts - AccountHistory - StorageHistory = Full Node
```
```text
2.14TB - 250GB - 240GB - 700GB = 950GB
```

## RPC support

As it was mentioned in the [pruning configuration chapter](./config.md#the-prune-section), there are several segments which can be pruned
independently of each other:
- Sender Recovery
- Transaction Lookup
- Receipts
- Account History
- Storage History

Pruning of each of these segments disables different RPC methods, because the historical data or lookup indexes
become unavailable.

### Full Node

The following tables describe RPC methods available in the full node.


#### `debug` namespace

| RPC                        | Note                                                       |
|----------------------------|------------------------------------------------------------|
| `debug_getRawBlock`        |                                                            |
| `debug_getRawHeader`       |                                                            |
| `debug_getRawReceipts`     | Only for the last 10064 blocks and Beacon Deposit Contract |
| `debug_getRawTransaction`  |                                                            |
| `debug_traceBlock`         | Only for the last 10064 blocks                             |
| `debug_traceBlockByHash`   | Only for the last 10064 blocks                             |
| `debug_traceBlockByNumber` | Only for the last 10064 blocks                             |
| `debug_traceCall`          | Only for the last 10064 blocks                             |
| `debug_traceCallMany`      | Only for the last 10064 blocks                             |
| `debug_traceTransaction`   | Only for the last 10064 blocks                             |


#### `eth` namespace

| RPC / Segment                             | Note                                                       |
|-------------------------------------------|------------------------------------------------------------|
| `eth_accounts`                            |                                                            |
| `eth_blockNumber`                         |                                                            |
| `eth_call`                                | Only for the last 10064 blocks                             |
| `eth_chainId`                             |                                                            |
| `eth_createAccessList`                    | Only for the last 10064 blocks                             |
| `eth_estimateGas`                         | Only for the last 10064 blocks                             |
| `eth_feeHistory`                          |                                                            |
| `eth_gasPrice`                            |                                                            |
| `eth_getBalance`                          | Only for the last 10064 blocks                             |
| `eth_getBlockByHash`                      |                                                            |
| `eth_getBlockByNumber`                    |                                                            |
| `eth_getBlockReceipts`                    | Only for the last 10064 blocks and Beacon Deposit Contract |
| `eth_getBlockTransactionCountByHash`      |                                                            |
| `eth_getBlockTransactionCountByNumber`    |                                                            |
| `eth_getCode`                             |                                                            |
| `eth_getFilterChanges`                    |                                                            |
| `eth_getFilterLogs`                       | Only for the last 10064 blocks and Beacon Deposit Contract |
| `eth_getLogs`                             | Only for the last 10064 blocks and Beacon Deposit Contract |
| `eth_getStorageAt`                        | Only for the last 10064 blocks                             |
| `eth_getTransactionByBlockHashAndIndex`   |                                                            |
| `eth_getTransactionByBlockNumberAndIndex` |                                                            |
| `eth_getTransactionByHash`                |                                                            |
| `eth_getTransactionCount`                 | Only for the last 10064 blocks                             |
| `eth_getTransactionReceipt`               | Only for the last 10064 blocks and Beacon Deposit Contract |
| `eth_getUncleByBlockHashAndIndex`         |                                                            |
| `eth_getUncleByBlockNumberAndIndex`       |                                                            |
| `eth_getUncleCountByBlockHash`            |                                                            |
| `eth_getUncleCountByBlockNumber`          |                                                            |
| `eth_maxPriorityFeePerGas`                |                                                            |
| `eth_mining`                              |                                                            |
| `eth_newBlockFilter`                      |                                                            |
| `eth_newFilter`                           |                                                            |
| `eth_newPendingTransactionFilter`         |                                                            |
| `eth_protocolVersion`                     |                                                            |
| `eth_sendRawTransaction`                  |                                                            |
| `eth_sendTransaction`                     |                                                            |
| `eth_sign`                                |                                                            |
| `eth_signTransaction`                     |                                                            |
| `eth_signTypedData`                       |                                                            |
| `eth_subscribe`                           |                                                            |
| `eth_syncing`                             |                                                            |
| `eth_uninstallFilter`                     |                                                            |
| `eth_unsubscribe`                         |                                                            |

#### `net` namespace

| RPC / Segment   |
|-----------------|
| `net_listening` |
| `net_peerCount` |
| `net_version`   |

#### `trace` namespace

| RPC / Segment                   | Note                           |
|---------------------------------|--------------------------------|
| `trace_block`                   | Only for the last 10064 blocks |
| `trace_call`                    | Only for the last 10064 blocks |
| `trace_callMany`                | Only for the last 10064 blocks |
| `trace_get`                     | Only for the last 10064 blocks |
| `trace_rawTransaction`          | Only for the last 10064 blocks |
| `trace_replayBlockTransactions` | Only for the last 10064 blocks |
| `trace_replayTransaction`       | Only for the last 10064 blocks |
| `trace_transaction`             | Only for the last 10064 blocks |

#### `txpool` namespace

| RPC / Segment        |
|----------------------|
| `txpool_content`     |
| `txpool_contentFrom` |
| `txpool_inspect`     |
| `txpool_status`      |


### Pruned Node

The following tables describe the requirements for prune segments, per RPC method:
- ✅ – if the segment is pruned, the RPC method still works
- ❌ - if the segment is pruned, the RPC method doesn't work anymore

#### `debug` namespace

| RPC / Segment              | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|----------------------------|-----------------|--------------------|----------|-----------------|-----------------|
| `debug_getRawBlock`        | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `debug_getRawHeader`       | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `debug_getRawReceipts`     | ✅               | ✅                  | ❌        | ✅               | ✅               |
| `debug_getRawTransaction`  | ✅               | ❌                  | ✅        | ✅               | ✅               |
| `debug_traceBlock`         | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `debug_traceBlockByHash`   | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `debug_traceBlockByNumber` | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `debug_traceCall`          | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `debug_traceCallMany`      | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `debug_traceTransaction`   | ✅               | ✅                  | ✅        | ❌               | ❌               |


#### `eth` namespace

| RPC / Segment                             | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|-------------------------------------------|-----------------|--------------------|----------|-----------------|-----------------|
| `eth_accounts`                            | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_blockNumber`                         | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_call`                                | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `eth_chainId`                             | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_createAccessList`                    | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `eth_estimateGas`                         | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `eth_feeHistory`                          | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_gasPrice`                            | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getBalance`                          | ✅               | ✅                  | ✅        | ❌               | ✅               |
| `eth_getBlockByHash`                      | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getBlockByNumber`                    | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getBlockReceipts`                    | ✅               | ✅                  | ❌        | ✅               | ✅               |
| `eth_getBlockTransactionCountByHash`      | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getBlockTransactionCountByNumber`    | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getCode`                             | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getFilterChanges`                    | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getFilterLogs`                       | ✅               | ✅                  | ❌        | ✅               | ✅               |
| `eth_getLogs`                             | ✅               | ✅                  | ❌        | ✅               | ✅               |
| `eth_getStorageAt`                        | ✅               | ✅                  | ✅        | ✅               | ❌               |
| `eth_getTransactionByBlockHashAndIndex`   | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getTransactionByBlockNumberAndIndex` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getTransactionByHash`                | ✅               | ❌                  | ✅        | ✅               | ✅               |
| `eth_getTransactionCount`                 | ✅               | ✅                  | ✅        | ❌               | ✅               |
| `eth_getTransactionReceipt`               | ✅               | ❌                  | ❌        | ✅               | ✅               |
| `eth_getUncleByBlockHashAndIndex`         | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getUncleByBlockNumberAndIndex`       | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getUncleCountByBlockHash`            | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_getUncleCountByBlockNumber`          | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_maxPriorityFeePerGas`                | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_mining`                              | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_newBlockFilter`                      | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_newFilter`                           | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_newPendingTransactionFilter`         | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_protocolVersion`                     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_sendRawTransaction`                  | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_sendTransaction`                     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_sign`                                | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_signTransaction`                     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_signTypedData`                       | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_subscribe`                           | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_syncing`                             | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_uninstallFilter`                     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `eth_unsubscribe`                         | ✅               | ✅                  | ✅        | ✅               | ✅               |

#### `net` namespace

| RPC / Segment   | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|-----------------|-----------------|--------------------|----------|-----------------|-----------------|
| `net_listening` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `net_peerCount` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `net_version`   | ✅               | ✅                  | ✅        | ✅               | ✅               |

#### `trace` namespace

| RPC / Segment                   | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|---------------------------------|-----------------|--------------------|----------|-----------------|-----------------|
| `trace_block`                   | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_call`                    | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_callMany`                | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_get`                     | ✅               | ❌                  | ✅        | ❌               | ❌               |
| `trace_rawTransaction`          | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_replayBlockTransactions` | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_replayTransaction`       | ✅               | ❌                  | ✅        | ❌               | ❌               |
| `trace_transaction`             | ✅               | ❌                  | ✅        | ❌               | ❌               |

#### `txpool` namespace

| RPC / Segment        | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|----------------------|-----------------|--------------------|----------|-----------------|-----------------|
| `txpool_content`     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `txpool_contentFrom` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `txpool_inspect`     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `txpool_status`      | ✅               | ✅                  | ✅        | ✅               | ✅               |
