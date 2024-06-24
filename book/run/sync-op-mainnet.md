# Sync OP Mainnet

To sync OP mainnet, the Bedrock datadir needs to be imported to use as starting point.
Blocks lower than the OP mainnet Bedrock fork, are built on the OVM and cannot be executed on the EVM.
For this reason, the chain segment from genesis until Bedrock, must be manually imported to circumvent
execution in reth's sync pipeline.

Importing OP mainnet Bedrock datadir requires exported data:

- Blocks [and receipts] below Bedrock
- State snapshot at first Bedrock block

## Manual Export Steps

See <https://github.com/testinprod-io/op-geth/pull/1>.

Output from running the command to export state, can also be downloaded from <https://datadirs.testinprod.io/world_trie_state_op_mainnet_b2c2b6e7edb919a0b856b9fd9aa02b11ead5305e63cdb33386babd82b9bc4cfe.jsonl.zst>.

## Manual Import Steps

### 1. Import Blocks

Imports a `.rlp` file of blocks.

Import of >100 million OVM blocks, from genesis to Bedrock, completes in 45 minutes.

```bash
./op-reth import-op <exported-blocks>
```

### 2. Import Receipts

This step is optional. To run a full node, skip this step. If however receipts are to be imported, the
corresponding transactions must already be imported (see [step 1](#1-import-blocks)).

Imports a `.rlp` file of receipts, that has been exported with command specified in
<https://github.com/testinprod-io/op-geth/pull/1> (command for exporting receipts uses custom RLP-encoding). 

Import of >100 million OVM receipts, from genesis to Bedrock, completes in 30 minutes.

```bash
./op-reth import-receipts-op <exported-receipts>
```

### 3. Import State

Imports a `.jsonl` state dump. The block at which the state dump is made, must be the latest block in
reth's database. This should be block 105 235 063, the first Bedrock block (see [step 1](#1-import-blocks)).

Import of >4 million OP mainnet accounts at Bedrock, completes in 10 minutes.

```bash
./op-reth init-state --chain optimism <state-dump>
```

## Sync from Bedrock to tip

Running the node with `--debug.tip <block-hash>`syncs the node without help from CL until a fixed tip. The
block hash can be taken from the latest block on <https://optimistic.etherscan.io>.

Use `op-node` to track the tip. Start `op-node` with `--syncmode=execution-layer`. If `op-node`'s RPC
connection to L1 is over localhost, `--l1.trustrpc` can be set to improve performance.