# Pruning

> WARNING: pruning and full node are experimental features of Reth, 
> and available only on `main` branch of the main repository now.

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

Pruning of each of these parts disables different RPC methods,  because the historical data or lookup indexes
become unavailable.

> TODO: `prune parts / RPC methods` table that shows which RPCs becomes unavailable when  certain parts of the data
> are pruned 