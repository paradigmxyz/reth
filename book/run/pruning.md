# Pruning

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
- Full Node – Reth node that has the latest state and historical data for only the last 128 blocks available
for querying in the same way as an archive node.

The node type that was chosen when first [running a node](./run-a-node.md) **can not** be changed after
the initial sync. Turning Archive into Pruned, or Pruned into Full is not supported.

## Modes
### Archive Node

Default mode, follow the steps from the previous chapter on [how to run on mainnet or official testnets](./mainnet.md).

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

### Pruned Node

To run Reth as a pruned node configured through a [custom configuration](./config.md#the-prune-section),
modify the `reth.toml` file and run Reth in the same way as archive node by following the steps from
the previous chapter on [how to run on mainnet or official testnets](./mainnet.md).

## Size

All numbers are as of August 2023 at block number 17.9M for mainnet.

### Archive

Archive node occupies at least 2.1TB.

You can track the growth of Reth archive node size with our
[public Grafana dashboard](https://reth.paradigm.xyz/d/2k8BXz24k/reth?orgId=1&refresh=30s&viewPanel=52).

### Full

Full node occupies 1TB at the peak, and slowly goes down to 920GB.

### Pruned

Different parts take up different amounts of disk space.
If pruned fully, this is the total freed space you'll get, per part: 

| Part               | Size  |
|--------------------|-------|
| Sender Recovery    | 70GB  |
| Transaction Lookup | 140GB |
| Receipts           | 240GB |
| Account History    | 230GB |
| Storage History    | 680GB |

## RPC support

As it was mentioned in the [pruning configuration chapter](./config.md#the-prune-section), there are several parts
which can be pruned independently of each other:
- Sender Recovery
- Transaction Lookup
- Receipts
- Account History
- Storage History

Pruning of each of these parts disables different RPC methods, because the historical data or lookup indexes
become unavailable.

The following tables describe the requirements for prune parts, per RPC method:
- ✅ – if the part is pruned, the RPC method still works
- ❌ - if the part is pruned, the RPC method doesn't work anymore

### `debug` namespace

| RPC / Part                 | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
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


### `eth` namespace

| RPC / Part                                | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
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

### `net` namespace

| RPC / Part      | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|-----------------|-----------------|--------------------|----------|-----------------|-----------------|
| `net_listening` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `net_peerCount` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `net_version`   | ✅               | ✅                  | ✅        | ✅               | ✅               |

### `trace` namespace

| RPC / Part                      | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|---------------------------------|-----------------|--------------------|----------|-----------------|-----------------|
| `trace_block`                   | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_call`                    | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_callMany`                | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_get`                     | ✅               | ❌                  | ✅        | ❌               | ❌               |
| `trace_rawTransaction`          | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_replayBlockTransactions` | ✅               | ✅                  | ✅        | ❌               | ❌               |
| `trace_replayTransaction`       | ✅               | ❌                  | ✅        | ❌               | ❌               |
| `trace_transaction`             | ✅               | ❌                  | ✅        | ❌               | ❌               |

### `txpool` namespace

| RPC / Part           | Sender Recovery | Transaction Lookup | Receipts | Account History | Storage History |
|----------------------|-----------------|--------------------|----------|-----------------|-----------------|
| `txpool_content`     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `txpool_contentFrom` | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `txpool_inspect`     | ✅               | ✅                  | ✅        | ✅               | ✅               |
| `txpool_status`      | ✅               | ✅                  | ✅        | ✅               | ✅               |
