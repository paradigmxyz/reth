# Sync OP Mainnet

To sync OP mainnet, bedrock state needs to be imported as a starting point. There are currently two ways:

* Minimal bootstrap **(recommended)**: only state snapshot at Bedrock block is imported without any OVM historical data.
* Full bootstrap **(not recommended)**: state, blocks and receipts are imported. *Not recommended for now: [storage consistency issue](https://github.com/paradigmxyz/reth/pull/11099) tldr: sudden crash may break the node

## Minimal bootstrap (recommended)

**The state snapshot at Bedrock block is required.** It can be exported from [op-geth](https://github.com/testinprod-io/op-erigon/blob/pcw109550/bedrock-db-migration/bedrock-migration.md#export-state) (**.jsonl**) or downloaded directly from [here](https://mega.nz/file/GdZ1xbAT#a9cBv3AqzsTGXYgX7nZc_3fl--tcBmOAIwIA5ND6kwc).

```sh
$ op-reth init-state --without-ovm --chain optimism --datadir op-mainnet world_trie_state.jsonl

$ op-reth node --chain optimism --datadir op-mainnet --debug.tip 0x098f87b75c8b861c775984f9d5dbe7b70cbbbc30fc15adb03a5044de0144f2d0 # block #125200000
```


## Full bootstrap (not recommended)

**Not recommended for now**: [storage consistency issue](https://github.com/paradigmxyz/reth/pull/11099) tldr: sudden crash may break the node.

### Import state 

To sync OP mainnet, the Bedrock datadir needs to be imported to use as starting point.
Blocks lower than the OP mainnet Bedrock fork, are built on the OVM and cannot be executed on the EVM.
For this reason, the chain segment from genesis until Bedrock, must be manually imported to circumvent
execution in reth's sync pipeline.

Importing OP mainnet Bedrock datadir requires exported data:

- Blocks [and receipts] below Bedrock
- State snapshot at first Bedrock block

### Manual Export Steps

The `op-geth` Bedrock datadir can be downloaded from <https://datadirs.optimism.io>.

To export the OVM chain from `op-geth`, clone the `testinprod-io/op-geth` repo and checkout
<https://github.com/testinprod-io/op-geth/pull/1>. Commands to export blocks, receipts and state dump can be
found in `op-geth/migrate.sh`.

### Manual Import Steps

#### 1. Import Blocks

Imports a `.rlp` file of blocks.

Import of >100 million OVM blocks, from genesis to Bedrock, completes in 45 minutes.

```bash
$ op-reth import-op --chain optimism <exported-blocks>
```

#### 2. Import Receipts

This step is optional. To run a full node, skip this step. If however receipts are to be imported, the
corresponding transactions must already be imported (see [step 1](#1-import-blocks)).

Imports a `.rlp` file of receipts, that has been exported with command specified in
<https://github.com/testinprod-io/op-geth/pull/1> (command for exporting receipts uses custom RLP-encoding). 

Import of >100 million OVM receipts, from genesis to Bedrock, completes in 30 minutes.

```bash
$ op-reth import-receipts-op --chain optimism <exported-receipts>
```

#### 3. Import State

Imports a `.jsonl` state dump. The block at which the state dump is made, must be the latest block in
reth's database. This should be block 105 235 063, the first Bedrock block (see [step 1](#1-import-blocks)).

Import of >4 million OP mainnet accounts at Bedrock, completes in 10 minutes.

```bash
$ op-reth init-state --chain optimism <state-dump>
```

## Sync from Bedrock to tip

Running the node with `--debug.tip <block-hash>`syncs the node without help from CL until a fixed tip. The
block hash can be taken from the latest block on <https://optimistic.etherscan.io>.

Use `op-node` to track the tip. Start `op-node` with `--syncmode=execution-layer` and `--l2.enginekind=reth`. If `op-node`'s RPC
connection to L1 is over localhost, `--l1.trustrpc` can be set to improve performance.
