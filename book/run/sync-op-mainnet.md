# Sync OP Mainnet

To sync OP mainnet, the Bedrock datadir needs to be imported to use ase starting point.
Blocks lower than the OP mainnet Bedrock fork, are built on the OVM and cannot be executed with EVM.
For this reason, the chain segment from genesis until Bedrock, must be manually imported to circumvent
execution in the pipeline.

Importing OP mainnet pre-bedrock requires exported data:

- Blocks and receipts below Bedrock
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

Imports a `.rlp` file of receipts, that has been exported with command to export receipts specified in 
<https://github.com/testinprod-io/op-geth/pull/1> (export command uses custom RLP-encoding). 

```bash
./op-reth import-receipts --chain optimism  <exported-receipts>
```

### 3. Import State

Imports a `.jsonl` state dump.

```bash
./op-reth init-state --chain optimism <state-dump>
```