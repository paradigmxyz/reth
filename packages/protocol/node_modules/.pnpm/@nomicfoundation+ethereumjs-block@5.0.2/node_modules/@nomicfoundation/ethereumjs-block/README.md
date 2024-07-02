# @ethereumjs/block

[![NPM Package][block-npm-badge]][block-npm-link]
[![GitHub Issues][block-issues-badge]][block-issues-link]
[![Actions Status][block-actions-badge]][block-actions-link]
[![Code Coverage][block-coverage-badge]][block-coverage-link]
[![Discord][discord-badge]][discord-link]

| Implements schema and functions related to Ethereum's block. |
| ------------------------------------------------------------ |

Note: this `README` reflects the state of the library from `v3.0.0` onwards. See `README` from the [standalone repository](https://github.com/ethereumjs/ethereumjs-block) for an introduction on the last preceding release.

## Installation

To obtain the latest version, simply require the project using `npm`:

```shell
npm install @ethereumjs/block
```

**Note:** If you want to work with `EIP-4844` related functionality, you will have additional manual installation steps for the **KZG setup**, see related section below.

## Usage

### Introduction

There are five static factories to instantiate a `Block`:

- `Block.fromBlockData(blockData: BlockData = {}, opts?: BlockOptions)`
- `Block.fromRLPSerializedBlock(serialized: Buffer, opts?: BlockOptions)`
- `Block.fromValuesArray(values: BlockBuffer, opts?: BlockOptions)`
- `Block.fromRPC(blockData: JsonRpcBlock, uncles?: any[], opts?: BlockOptions)`
- `Block.fromEthersProvider(provider: ethers.providers.JsonRpcProvider | string, blockTag: string | bigint, opts: BlockOptions)`

For `BlockHeader` instantiation analog factory methods exists, see API docs linked below.

Instantiation Example:

```typescript
import { BlockHeader } from '@ethereumjs/block'

const headerData = {
  number: 15,
  parentHash: '0x6bfee7294bf44572b7266358e627f3c35105e1c3851f3de09e6d646f955725a7',
  difficulty: 131072,
  gasLimit: 8000000,
  timestamp: 1562422144,
}
const header = BlockHeader.fromHeaderData(headerData)
```

Properties of a `Block` or `BlockHeader` object are frozen with `Object.freeze()` which gives you enhanced security and consistency properties when working with the instantiated object. This behavior can be modified using the `freeze` option in the constructor if needed.

API Usage Example:

```typescript
try {
  await block.validateData()
  // Block data validation has passed
} catch (err) {
  // handle errors appropriately
}
```

### EIP-1559 Blocks

This library supports the creation of [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) compatible blocks starting with `v3.3.0`. For this to work a Block needs to be instantiated with a Hardfork greater or equal to London (`Hardfork.London`).

```typescript
import { Block } from '@ethereumjs/block'
import { Chain, Common, Hardfork } from '@ethereumjs/common'
const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.London })

const block = Block.fromBlockData(
  {
    header: {
      baseFeePerGas: BigInt(10),
      gasLimit: BigInt(100),
      gasUsed: BigInt(60),
    },
  },
  { common }
)

// Base fee will increase for next block since the
// gas used is greater than half the gas limit
block.header.calcNextBaseFee().toNumber() // 11

// So for creating a block with a matching base fee in a certain
// chain context you can do:

const blockWithMatchingBaseFee = Block.fromBlockData(
  {
    header: {
      baseFeePerGas: parentHeader.calcNextBaseFee(),
      gasLimit: BigInt(100),
      gasUsed: BigInt(60),
    },
  },
  { common }
)
```

EIP-1559 blocks have an extra `baseFeePerGas` field (default: `BigInt(7)`) and can encompass `FeeMarketEIP1559Transaction` txs (type `2`) (supported by `@ethereumjs/tx` `v3.2.0` or higher) as well as `Transaction` legacy txs (internal type `0`) and `AccessListEIP2930Transaction` txs (type `1`).

### EIP-4895 Beacon Chain Withdrawals Blocks (experimental)

Starting with the `v4.1.0` release there is (experimental) support for [EIP-4895](https://eips.ethereum.org/EIPS/eip-4895) beacon chain withdrawals. Withdrawals support can be activated by initializing a respective `Common` object and then use the `withdrawals` data option to pass in system-level withdrawal operations together with a matching `withdrawalsRoot` (mandatory when `EIP-4895` is activated) along Block creation, see the following example:

```typescript
import { Block } from '@ethereumjs/block'
import { Common, Chain } from '@ethereumjs/common'
import { Address } from '@ethereumjs/util'
import type { WithdrawalData } from '@ethereumjs/util'

const common = new Common({ chain: Chain.Mainnet, eips: [4895] })

const withdrawal = <WithdrawalData>{
  index: BigInt(0),
  validatorIndex: BigInt(0),
  address: new Address(Buffer.from('20'.repeat(20), 'hex')),
  amount: BigInt(1000),
}

const block = Block.fromBlockData(
  {
    header: {
      withdrawalsRoot: Buffer.from(
        '69f28913c562b0d38f8dc81e72eb0d99052444d301bf8158dc1f3f94a4526357',
        'hex'
      ),
    },
    withdrawals: [withdrawal],
  },
  {
    common,
  }
)
```

Validation of the withdrawals trie can be manually triggered with the newly introduced async `Block.validateWithdrawalsTrie()` method.

### EIP-4844 Shard Blob Transaction Blocks (experimental)

This library supports an experimental version of the blob transaction type introduced with [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844) as being specified in the [01d3209](https://github.com/ethereum/EIPs/commit/01d320998d1d53d95f347b5f43feaf606f230703) EIP version from February 8, 2023 and deployed along `eip4844-devnet-4` (January 2023) starting with `v4.2.0`.

#### Initialization

To create blocks which include blob transactions you have to active EIP-4844 in the associated `@ethereumjs/common` library:

```typescript
import { Common, Chain, Hardfork } from '@ethereumjs/common'

const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Shanghai, eips: [4844] })
```

**Note:** Working with blob transactions needs a manual KZG library installation and global initialization, see [KZG Setup](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/tx/README.md#kzg-setup) for instructions.

### Consensus Types

The block library supports the creation as well as consensus format validation of PoW `ethash` and PoA `clique` blocks (so e.g. do specific `extraData` checks on Clique/PoA blocks).

Consensus format validation logic is encapsulated in the semi-private `BlockHeader._consensusFormatValidation()` method called from the constructor. If you want to add your own validation logic you can overwrite this method with your own rules.

Note: Starting with `v4` consensus validation itself (e.g. Ethash verification) has moved to the `Blockchain` package.

### Ethash/PoW

An Ethash/PoW block can be instantiated as follows:

```typescript
import { Block } from '@ethereumjs/block'
import { Chain, Common } from '@ethereumjs/common'
const common = new Common({ chain: Chain.Mainnet })
console.log(common.consensusType()) // 'pow'
console.log(common.consensusAlgorithm()) // 'ethash'
const block = Block.fromBlockData({}, { common })
```

To calculate the difficulty when creating the block pass in the block option `calcDifficultyFromHeader` with the preceding (parent) `BlockHeader`.

### Clique/PoA (since v3.1.0)

A clique block can be instantiated as follows:

```typescript
import { Block } from '@ethereumjs/block'
import { Chain, Common } from '@ethereumjs/common'
const common = new Common({ chain: Chain.Goerli })
console.log(common.consensusType()) // 'poa'
console.log(common.consensusAlgorithm()) // 'clique'
const block = Block.fromBlockData({}, { common })
```

For sealing a block on instantiation you can use the `cliqueSigner` constructor option:

```typescript
const cliqueSigner = Buffer.from('PRIVATE_KEY_HEX_STRING', 'hex')
const block = Block.fromHeaderData(headerData, { cliqueSigner })
```

Additionally there are the following utility methods for Clique/PoA related functionality in the `BlockHeader` class:

- `BlockHeader.cliqueSigHash()`
- `BlockHeader.cliqueIsEpochTransition(): boolean`
- `BlockHeader.cliqueExtraVanity(): Buffer`
- `BlockHeader.cliqueExtraSeal(): Buffer`
- `BlockHeader.cliqueEpochTransitionSigners(): Address[]`
- `BlockHeader.cliqueVerifySignature(signerList: Address[]): boolean`
- `BlockHeader.cliqueSigner(): Address`

See the API docs for detailed documentation. Note that these methods will throw if called in a non-Clique/PoA context.

### Casper/PoS (since v3.5.0)

Merge-friendly Casper/PoS blocks have been introduced along with the `v3.5.0` release. Proof-of-Stake compatible execution blocks come with their own set of header field simplifications and associated validation rules. The difficulty is set to `0` since not relevant anymore, just to name an example. For a full list of changes see [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675).

You can instantiate a Merge/PoS block like this:

```typescript
import { Block } from '@ethereumjs/block'
import { Chain, Common, Hardfork } from '@ethereumjs/common'
const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Merge })
const block = Block.fromBlockData(
  {
    // Provide your block data here or use default values
  },
  { common }
)
```

## API

### Docs

Generated TypeDoc API [Documentation](./docs/README.md)

### BigInt Support

Starting with v4 the usage of [BN.js](https://github.com/indutny/bn.js/) for big numbers has been removed from the library and replaced with the usage of the native JS [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) data type (introduced in `ES2020`).

Please note that number-related API signatures have changed along with this version update and the minimal build target has been updated to `ES2020`.

## Testing

Tests in the `tests` directory are partly outdated and testing is primarily done by running the `BlockchainTests` from within the [@ethereumjs/vm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/vm) package.

To avoid bloating this repository with [ethereum/tests](https://github.com/ethereum/tests) JSON files, we usually copy specific JSON files and wrap them with some metadata (source, date, commit hash). There's a helper to aid in that process and can be found at [wrap-ethereum-test.sh](https://github.com/ethereumjs/ethereumjs-monorepo/blob/master/packages/block/scripts/wrap-ethereum-test.sh).

## EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices. If you want to join for work or carry out improvements on the libraries, please review our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html) first.

## License

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[block-npm-badge]: https://img.shields.io/npm/v/@ethereumjs/block.svg
[block-npm-link]: https://www.npmjs.com/package/@ethereumjs/block
[block-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20block?label=issues
[block-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+block"
[block-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/Block/badge.svg
[block-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22Block%22
[block-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=block
[block-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/block
