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

Note! Requires running in debug mode (TODO: <https://github.com/paradigmxyz/reth/issues/7650>).

```bash
./op-reth import-op <exported-blocks>
```

### 2. Import Receipts

This step is optional. To run a full node, skip this step. If however receipts are to be imported, the
corresponding transactions must already be imported (see [step 1](#1-import-blocks)).

Imports a `.rlp` file of receipts, that has been exported with command specified in
<https://github.com/testinprod-io/op-geth/pull/1> (command for exporting receipts uses custom RLP-encoding). 

```bash
./op-reth import-receipts --chain optimism <exported-receipts>
```

### 3. Import State

Imports a `.jsonl` state dump. The block at which the state dump is made, must be the latest block in
reth's database.

```bash
./op-reth init-state --chain optimism <state-dump>
```