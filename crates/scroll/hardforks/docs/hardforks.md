---
section: technology
date: Last Modified
title: "Scroll Upgrades"
lang: "en"
permalink: "technology/overview/scroll-upgrades"
---

As the team continues to progress on Scroll's roadmap, we will be upgrading the Scroll network to include new features
and improvements.

The following contracts are used to initiate upgrades and execute upgrades after the two-week timelock period:

| Contract           | Network  | Address                                                                                                                   |
|--------------------|----------|---------------------------------------------------------------------------------------------------------------------------|
| L1 Scroll Multisig | Ethereum | [`0xEfc9D1096fb65c832207E5e7F13C2D1102244dbe`](https://etherscan.io/address/0xEfc9D1096fb65c832207E5e7F13C2D1102244dbe)   |
| L1 Timelock        | Ethereum | [`0x1A658B88fD0a3c82fa1a0609fCDbD32e7dd4aB9C`](https://etherscan.io/address/0x1A658B88fD0a3c82fa1a0609fCDbD32e7dd4aB9C)   |
| L2 Scroll Multisig | Scroll   | [`0xEfc9D1096fb65c832207E5e7F13C2D1102244dbe`](https://scrollscan.com/address/0xEfc9D1096fb65c832207E5e7F13C2D1102244dbe) |
| L2 Timelock        | Scroll   | [`0xf6069DB81239E5194bb53f83aF564d282357bc99`](https://scrollscan.com/address/0xf6069DB81239E5194bb53f83aF564d282357bc99) |

You can join our [Telegram channel for technical updates](https://t.me/scroll_tech_updates), which includes future
upgrade announcements and on-chain operation events.

## `DarwinV2` Upgrade

### Overview

During internal testing, we identified that blocks may not always be compressible under certain conditions, which leads
to unprovable chunks and batches.
To fix this issue, a minor upgrade has been conducted so that uncompressed blobs will be enabled when this special case
is detected.

### Timeline

As this is a security related patch, we bypassed the 7-day timelock mechanism.

- **Scroll Sepolia**: August 28th, 2024
- **Scroll Mainnet**: September 2nd, 2024

### Compatibility

#### Sequencer and Follower Nodes (l2geth)

The new node version is `v5.7.0`. See
the [release notes](https://github.com/scroll-tech/go-ethereum/releases/tag/scroll-v5.7.0) for more information.

This upgrade does not change Scroll's state transition function, so it is backward compatible. However, the format of
the batch data committed to Ethereum changes. As a result, nodes that enabled rollup verification (`--rollup.verify`)
must upgrade to be able to follow the chain.

#### Dapps and Indexers

A change has been implemented to Scroll Mainnet to enhance sequencer throughput, which adjusted the maximum reorg depth
to 17 blocks. Previously, the system performed thorough capacity checks within the signer thread to determine whether
transactions exceed the circuit limit. While this ensures that all transactions within a block are compliant, it also
requires additional CPU resources.
We introduced a new circuit capacity checking scheme on Mainnet. The sequencer thread now will continue to perform
capacity checks, but in a more approximate manner. In parallel, 16 worker threads will accurately verify the capacity of
previous blocks. As a result, a reorg could occur with a maximum depth of 17 blocks, although the likelihood of this is
low.

For indexers, the `BatchHeader` version has been upgraded to 4. This is backward compatible (the only exception is for
developers decoding the blob payload, which has changed slightly).

## Darwin Upgrade

### Overview

This upgrade will reduce gas fees by 34% by using a single aggregated proof for multiple batches, eliminating the need
to finalize each batch individually.

- Darwin uses a new [V3 batch codec](https://github.com/scroll-tech/da-codec/tree/main/encoding/codecv3).
- In addition to the previous notions of `chunk` and `batch`, we have introduced a new concept called `bundle`.
    - `Chunk`: A unit of zkEVM proving, consisting of a list of L2 blocks.
    - `Batch`: A collection of chunks encoded into one EIP-4844 blob, serving as the unit of Data Availability.
    - `Bundle`: A series of batches that functions as the unit of finalization.

  The main difference compared to Curie is that Scroll will now finalize multiple batches using a single aggregated
  bundle proof.

- The on-chain bundle proof verifier uses a new public input layout.

### Timeline

- **Scroll Sepolia**
    - Network Upgrade: August 14th, 2024
- **Scroll Mainnet**
    - Upgrade Initiation: August 5th, 2024
    - Timelock Completion & Upgrade: August 21st, 2024

### Technical Details

#### Contract Changes

*Note: Since the previous Curie upgrade, we have migrated the Scroll contracts to a new repo
at [scroll-contracts](https://github.com/scroll-tech/scroll-contracts).*

The code changes for this upgrade are implemented in [this PR](https://github.com/scroll-tech/scroll-contracts/pull/4).
The key changes are as follows:

- We have introduced a new `BatchHeaderV3Codec`.
- We have changed how messages are processed in the `L1MessageQueue` contract. Prior to Darwin, we would process
  messages when a batch is finalized. After Darwin, most of this processing is moved to the commit step.
- We have introduced a new public input format for bundle proofs. This is implemented in a new contract
  `IZkEvmVerifierV2`, which is in turn added to `MultipleVersionRollupVerifier`.
- In the `ScrollChain` contract `version=3` batches will now be committed through a new function called
  `commitBatchWithBlobProof`. Bundles will be finalized using a new function called `finalizeBundleWithProof`.

See the contract [release notes](https://github.com/scroll-tech/scroll-contracts/releases/tag/v1.0.0) for more
information.

#### Node Changes

The new node version is `v5.6.0`. See
the [release notes](https://github.com/scroll-tech/go-ethereum/releases/tag/scroll-v5.6.0) for more information.

The main changes are:

- Implementation of timestamp-based hard forks.
- Processing V3 batch codec in rollup-verifier.

#### zkEVM circuit changes

The new version of zkevm circuits is `v0.12.0`.
See [here](https://github.com/scroll-tech/zkevm-circuits/releases/tag/v0.12.0) for the release log.

We have introduced a `RecursionCircuit` that will bundle multiple sequential batches by recursively aggregating the
SNARKs from the `BatchCircuit` (previously `AggregationCircuit`). The previously 5 layer proving system is now 7 layers
as we introduce:

- 6th Layer (layer5): `RecursionCircuit` that recursively aggregates `BatchCircuit` SNARKs.
- 7th Layer (layer6): `CompressionCircuit` that compresses the `RecursionCircuit` SNARK and produce an EVM-verifiable
  validity proof.

The public input to the `BatchCircuit` is now context-aware of the “previous” `batch`, which allows us to implement the
recursion scheme we adopted (
described [here](https://scrollzkp.notion.site/Upgrade-4-Darwin-Documentation-05a3ecb59e9d4f288254701f8c888173)
in-depth).

#### Audits

- TrailofBits: coming soon!

### Compatibility

#### Sequencer and Follower Nodes (l2geth)

This upgrade does not alter the state transition function and is therefore backward-compatible. However, we strongly
recommend node operators to upgrade to [v5.6.0](https://github.com/scroll-tech/go-ethereum/releases/tag/scroll-v5.6.0).

#### Dapps and Indexers

There are some major changes to how we commit and finalize batches after Darwin.

- Batches will be encoded using the
  new [V3 batch codec](https://github.com/scroll-tech/da-codec/tree/main/encoding/codecv3). This version adds two new
  fields:
    1. `lastBlockTimestamp` (the timestamp of the last block in this batch).
    2. `blobDataProof` (the KZG challenge point evaluation proof).

  This version removes `skippedL1MessageBitmap`. There will be no changes to how the blob data is encoded and
  compressed.
- Batches will be committed using the `commitBatchWithBlobProof` function (instead of the previous `commitBatch`).

  New function signature:

    ```solidity
    function commitBatchWithBlobProof(uint8 _version, bytes calldata _parentBatchHeader, bytes[] memory _chunks, bytes calldata _skippedL1MessageBitmap, bytes calldata _blobDataProof)
    ```

- Batches will be finalized using the `finalizeBundleWithProof` function (instead of the previous
  `finalizeBatchWithProof4844`).

  New function signature:

    ```solidity
    function finalizeBundleWithProof(bytes calldata _batchHeader, bytes32 _postStateRoot, bytes32 _withdrawRoot, bytes calldata _aggrProof)
    ```

- The semantics of the `FinalizeBatch` event will change: It will now mean that all batches between the last finalized
  batch and the event’s `_batchIndex` have been finalized. The event’s stateRoot and withdrawRoot values belong to the
  last finalized batch in the bundle. Finalized roots for intermediate batches are no longer available.

  The semantics of the `CommitBatch` and `RevertBatch` events will not change.

Recommendations:

- Indexers that decode committed batch data should be adjusted to use the new codec and the new function signature.
- Indexers that track batch finalization status should be adjusted to consider the new event semantics.

## Curie Upgrade

### Overview

This significant upgrade will reduce gas fees on the Scroll chain by 1.5x. Highlights include:

- Compresses the data stored in blobs using the [zstd](https://github.com/scroll-tech/da-codec/tree/main/libzstd)
  algorithm. This compression reduces the data size, allowing each blob to store more transactions, thereby reducing
  data availability cost per transaction.
- Adopts a modified version of the EIP-1559 pricing model which is compatible with the EIP-1559 transaction interface,
  bringing beneftis such as more accurate transaction pricing and a more predictable and stable fee structure.
- Support for new EVM opcodes `TLOAD`, `TSTORE`, and `MCOPY`. Users can safely use the latest Solidity compiler version
  `0.8.26` to build the contracts.
- Introduces a dynamic block time. During periods of traffic congestion, a block will be packed when the number of
  transactions reaches the circuit limit instead of waiting for the 3-second interval.

### Timeline

- **Scroll Sepolia**
    - Network Upgrade: June 17th, 2024
- **Scroll Mainnet**
    - Upgrade Initiation: June 20th, 2024
    - Timelock Completion & Upgrade: July 3rd, 2024

### Technical Details

#### Contract Changes

The code changes for this upgrade are documented in the following PRs:

- [Accept compressed batches](https://github.com/scroll-tech/scroll/pull/1317)
- [Update `L1GasPriceOracle`](https://github.com/scroll-tech/scroll/pull/1343)
- [Change `MAX_COMMIT_SCALAR` and `MAX_BLOB_SCALAR` to 1e18](https://github.com/scroll-tech/scroll/pull/1354)
- [Remove batch index check when updating a verifier](https://github.com/scroll-tech/scroll/pull/1372)

The main changes are as follows:

- The rollup contract (`ScrollChain`) will now accept batches with both versions 1 and
  2. [Version 1](https://github.com/scroll-tech/da-codec/tree/main/encoding/codecv1) is used for uncompressed blobs (
  pre-Curie), while [version 2](https://github.com/scroll-tech/da-codec/tree/main/encoding/codecv2) is used for
  compressed blobs (post-Curie).
- The `L1GasPriceOracle` contract will be updated to change the data fee formula to account for blob DA, providing a
  more accurate estimation of DA costs:
    - Original formula: `(l1GasUsed(txRlp) + overhead) * l1BaseFee * scalar`
    - New formula: `l1BaseFee * commitScalar + len(txRlp) * l1BlobBaseFee * blobScalar`

#### Node Changes

The new node version is `v5.5.0`. See
the [release notes](https://github.com/scroll-tech/go-ethereum/releases/tag/scroll-v5.5.0) for the list of changes.

#### zkEVM circuit changes

The new version of zkevm circuits is `v0.11.4`.
See [here](https://github.com/scroll-tech/zkevm-circuits/releases/tag/v0.11.4) for the release log.

#### Audits

- TrailofBits: coming soon!
- [Zellic](https://github.com/Zellic/publications/blob/master/Scroll%20zkEVM%20-%20Zellic%20Audit%20Report.pdf)

### Compatibility

#### Sequencer and Follower Nodes (l2geth)

This upgrade is a hard fork, introducing the `TLOAD`, `TSTORE`, and `MCOPY` opcodes. Operators running an `l2geth` node
are required to upgrade before the hard fork block. For more information, see
the [node release note](https://github.com/scroll-tech/go-ethereum/releases/tag/scroll-v5.4.2).

#### Dapps and Indexers

For dApps, this upgrade is backward compatible. Developers should adjust the gas fee settings to incorporate the
EIP-1559 pricing model. Note that dApps can no longer rely on the fixed 3-second block time in the application logic.

For indexers, the [data format](https://docs.scroll.io/en/technology/chain/rollup/#codec) remains the same. The will be
however changes to the data content:

- The `version` field in `BatchHeader` will be changed to 2 since Curie block.
- The data stored in blob will be compressed and can be decompressed
  by [zstd v1.5.6](https://github.com/facebook/zstd/releases/tag/v1.5.6).

## Bernoulli Upgrade

### Overview

This upgrade features a significant reduction in transaction costs by introducing support for EIP-4844 data blobs and
supporting the SHA2-256 precompile.

### Timeline

- **Scroll Sepolia**
    - Network Upgrade: April 15th, 2024
- **Scroll Mainnet**
    - Upgrade Initiation: April 15th, 2024
    - Timelock Completion & Upgrade: April 29th, 2024

### Technical Details

#### Contract changes

The contract changes for this upgrade are in [this PR](https://github.com/scroll-tech/scroll/pull/1179), along with the
audit
fixes [here](https://github.com/scroll-tech/scroll/pulls?q=is%3Apr+created%3A2024-04-10..2024-04-11+fix+in%3Atitle+label%3Abug).
The main changes are as follows:

- `ScrollChain` now accepts batches with either calldata or blob encoding in `commitBatch`.
- `ScrollChain` now supports finalizing blob-encoded batches through `finalizeBatchWithProof4844`.
- `MultipleVersionRollupVerifier` can now manage different on-chain verifiers for each batch encoding version.

#### Node changes

The new node version is `v5.3.0`. See [here](https://github.com/scroll-tech/go-ethereum/releases/tag/scroll-v5.3.0) for
the release log.

#### zkEVM circuit changes

The new version of zkevm circuits is `v0.10.3`.
See [here](https://github.com/scroll-tech/zkevm-circuits/releases/tag/v0.10.3) for the release log.

#### Audits

- [OpenZeppelin](https://blog.openzeppelin.com/scroll-eip-4844-support-audit)
- [TrailofBits](https://github.com/trailofbits/publications/blob/master/reviews/2024-04-scroll-4844-blob-securityreview.pdf)

### Compatibility

#### Sequencer and follower nodes (l2geth)

This upgrade is a hard fork as it introduces the new blob data type and the SHA2-256 precompiled contract. Operators
running an `l2geth` node are required to upgrade before the hard fork block. See
the [node releases](https://github.com/scroll-tech/go-ethereum/releases) for more information.

#### Indexers and Bridges

This upgrade changes the format that Scroll uses to publish data to Ethereum. Projects that rely on this data should
carefully review [the new data format](/en/technology/chain/rollup/#codec), and check whether their decoders need to be
adjusted. A summary of the new format:

- The format of [
  `BlockContext`](https://github.com/scroll-tech/scroll/blob/5362e28f744093495c1c09a6b68fc96a3264278b/common/types/encoding/codecv1/codecv1.go#L125)
  will not change.
- `Chunks`
  will [no longer include](https://github.com/scroll-tech/scroll/blob/5362e28f744093495c1c09a6b68fc96a3264278b/common/types/encoding/codecv1/codecv1.go#L162)
  the L2 transaction data. This will instead
  be [stored in a blob](https://github.com/scroll-tech/scroll/blob/5362e28f744093495c1c09a6b68fc96a3264278b/common/types/encoding/codecv1/codecv1.go#L284)
  attached to the `commitBatch` transaction.
- `BatchHeader` now contains one new field, [
  `BlobVersionedHash`](https://github.com/scroll-tech/scroll/blob/5362e28f744093495c1c09a6b68fc96a3264278b/common/types/encoding/codecv1/codecv1.go#L405).

#### Provers

This upgrade involves a breaking change in [zkevm-circuits](https://github.com/scroll-tech/zkevm-circuits). Operators
running a prover node are required to upgrade.

## Bridge Upgrade

### Overview

To reduce bridging costs, we implemented several gas optimizations on our bridge and rollup contract suite. The
optimization techniques used include the following:

- We will now use constants to store some companion contract addresses, instead of using storage variables. This is
  possible since these values should (almost) never change. With this change we can save on a few storage load
  operations.
- We updated the intrinsic gas estimation in `L1MessageQueue` to use a simple upper bound instead of an exact
  calculation. The two results will be similar for most bridge transactions but the new implementation is significantly
  cheaper.
- We merged two contracts `L1MessageQueue` and `L2GasPriceOracle` to save on call costs from one contract to the other.

### Timeline

- **Scroll Sepolia:**
    - Network Upgrade: January 19, 2024
- **Scroll Mainnet:**
    - Upgrade Initiation: February 7, 2024
    - Timelock Completion & Upgrade: February 21, 2024

### Technical Details

#### Code Changes

- [Bridge Cost Optimization](https://github.com/scroll-tech/scroll/pull/1011)
- [Audit Fixes](https://github.com/scroll-tech/scroll/pulls?q=OZ+is%3Apr+created%3A2024-01-27..2024-02-10)
- [Previously deployed version](https://github.com/scroll-tech/scroll/tree/ff380141a8cbcc214dc65f17ffa44faf4be646b6) (
  commit `ff380141a8cbcc214dc65f17ffa44faf4be646b6`)
- [Version deployed](https://github.com/scroll-tech/scroll/tree/6030927680a92d0285c2c13e6bb27ed27d1f32d1) (commit
  `6030927680a92d0285c2c13e6bb27ed27d1f32d1`)

#### Audits

- [OpenZeppelin](https://blog.openzeppelin.com/scroll-bridge-gas-optimizations-audit)

#### List of Changes

**Changes to L1 contracts:**

- In `ScrollChain`, change `messageQueue` and `verifier` to `immutable`.
- In `L1ScrollMessenger`, change `counterpart`, `rollup`, and `messageQueue` to `immutable`.
- In all token gateways, change `counterpart`, `router`, and `messenger` to `immutable`.
- Merge `L1MessageQueue` and `L2GasPriceOracle` into a single contract `L1MessageQueueWithGasPriceOracle` (deployed on
  the same address as the previous `L1MessageQueue`). In this contract, we also change `messenger` and `scrollChain` to
  `immutable`, and simplify `calculateIntrinsicGasFee`.

**Changes to L2 contracts:**

- In `L2ScrollMessenger`, change `counterpart` to `immutable`.
- In all token gateways, change `counterpart`, `router`, and `messenger` to `immutable`.

**Contracts affected:**

- **L1:** `L1MessageQueue`, `L2GasPriceOracle`, `ScrollChain`, `L1WETHGateway`, `L1StandardERC20Gateway`,
  `L1GatewayRouter`, `L1ScrollMessenger`, `L1CustomERC20Gateway`, `L1ERC721Gateway`, `L1ERC1155Gateway`.
- **L2:** `L2ScrollMessenger`, `L2WETHGateway`, `L2StandardERC20Gateway`, `L2GatewayRouter`, `L2CustomERC20Gateway`,
  `L2ERC721Gateway`, `L2ERC1155Gateway`.

#### Compatibility

##### Sequencer and follower nodes (l2geth)

Operators running an `l2geth` node do not need to upgrade. The changes in this upgrade will not affect `l2geth`.

##### Dapps and indexers

Dapps and indexers (and similar off-chain infrastructure) that query contracts or rely on contract interfaces would, in
most cases, not need to be changed. The majority of the contract changes are internal and/or backward compatible.

If your application depends on [
`L2GasPriceOracle`](https://etherscan.io/address/0x987e300fDfb06093859358522a79098848C33852) to monitor how Scroll keeps
track of the L2 gas price on L1, from the upgrade block number you will need to start monitoring [
`L1MessageQueueWithGasPriceOracle`](https://etherscan.io/address/0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B).

The original gas price oracle contract will be deprecated: it will no longer be updated or used by the Scroll bridge.

- Ethereum:
    - `L2GasPriceOracle`: [
      `0x987e300fDfb06093859358522a79098848C33852`](https://etherscan.io/address/0x987e300fDfb06093859358522a79098848C33852)
    - `L1MessageQueueWithGasPriceOracle`: [
      `0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B`](https://etherscan.io/address/0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B)
- Sepolia:
    - `L2GasPriceOracle`: [
      `0x247969F4fad93a33d4826046bc3eAE0D36BdE548`](https://sepolia.etherscan.io/address/0x247969F4fad93a33d4826046bc3eAE0D36BdE548)
    - `L1MessageQueueWithGasPriceOracle`: [
      `0xF0B2293F5D834eAe920c6974D50957A1732de763`](https://sepolia.etherscan.io/address/0xF0B2293F5D834eAe920c6974D50957A1732de763)