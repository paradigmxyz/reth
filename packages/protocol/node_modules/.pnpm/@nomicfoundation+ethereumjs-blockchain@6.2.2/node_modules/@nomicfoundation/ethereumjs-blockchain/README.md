# @ethereumjs/blockchain

[![NPM Package][blockchain-npm-badge]][blockchain-npm-link]
[![GitHub Issues][blockchain-issues-badge]][blockchain-issues-link]
[![Actions Status][blockchain-actions-badge]][blockchain-actions-link]
[![Code Coverage][blockchain-coverage-badge]][blockchain-coverage-link]
[![Discord][discord-badge]][discord-link]

| A module to store and interact with blocks. |
| ------------------------------------------- |

Note: this `README` reflects the state of the library from `v5.0.0` onwards. See `README` from the [standalone repository](https://github.com/ethereumjs/ethereumjs-blockchain) for an introduction on the last preceding release.

## Installation

To obtain the latest version, simply require the project using `npm`:

```shell
npm install @ethereumjs/blockchain
```

## Usage

### Introduction

The `Blockchain` package represents an Ethereum-compatible blockchain storing a sequential chain of [@ethereumjs/block](../block) blocks and holding information about the current canonical head block as well as the context the chain is operating in (e.g. the hardfork rules the current head block adheres to).

New blocks can be added to the blockchain. Validation ensures that the block format adheres to the given chain rules (with the `Blockchain.validateBlock()` function) and consensus rules (`Blockchain.consensus.validateConsensus()`).

The library also supports reorg scenarios e.g. by allowing to add a new block with `Blockchain.putBlock()` which follows a different canonical path to the head than given by the current canonical head block.

## Example

The following is an example to iterate through an existing Geth DB (needs `level` to be installed separately).

This module performs write operations. Making a backup of your data before trying it is recommended. Otherwise, you can end up with a compromised DB state.

```typescript
import { Blockchain } from '@ethereumjs/blockchain'
import { Chain, Common } from '@ethereumjs/common'

const { Level } = require('level')

const gethDbPath = './chaindata' // Add your own path here. It will get modified, see remarks.

const common = new Common({ chain: Chain.Ropsten })
const db = new Level(gethDbPath)
// Use the safe static constructor which awaits the init method
const blockchain = Blockchain.create({ common, db })

blockchain.iterator('i', (block) => {
  const blockNumber = block.header.number.toString()
  const blockHash = block.hash().toString('hex')
  console.log(`Block ${blockNumber}: ${blockHash}`)
})
```

**WARNING**: Since `@ethereumjs/blockchain` is also doing write operations on the DB for safety reasons only run this on a copy of your database, otherwise this might lead to a compromised DB state.

### Consensus

Starting with v6 there is a dedicated consensus class for each type of supported consensus, `Ethash`, `Clique` and `Casper` (PoS, this one is rather the do-nothing part of `Casper` and letting the respective consensus/beacon client do the hard work! 🙂). Each consensus class adheres to a common interface `Consensus` implementing the following five methods in a consensus-specific way:

- `genesisInit(genesisBlock: Block): Promise<void>`
- `setup(): Promise<void>`
- `validateConsensus(block: Block): Promise<void>`
- `validateDifficulty(header: BlockHeader): Promise<void>`
- `newBlock(block: Block, commonAncestor?: BlockHeader, ancientHeaders?: BlockHeader[]): Promise<void>`

#### Custom Conensus Algorithms

Also part of V6, you can also create a custom consensus class implementing the above interface and pass it into the `Blockchain` constructor using the `consensus` option at instantiation. See [this test script](https://github.com/ethereumjs/ethereumjs-monorepo/blob/master/packages/blockchain/test/customConsensus.spec.ts) for a complete example of how write and use a custom consensus implementation.

Note, if you construct a blockchain with a custom consensus implementation, transition checks for switching from PoW to PoS are disabled so defining a merge hardfork will have no impact on the consensus mechanism defined for the chain.

## Custom Genesis State

Starting with v6 responsibility for setting up a custom genesis state moved from the [Common](../common/) library to the `Blockchain` package, see PR [#1924](https://github.com/ethereumjs/ethereumjs-monorepo/pull/1924) for some work context.

A genesis state can be set along `Blockchain` creation by passing in a custom `genesisBlock` and `genesisState`. For `mainnet` and the official test networks like `sepolia` or `goerli` genesis is already provided with the block data coming from `@ethereumjs/common`. The genesis state is being integrated in the `Blockchain` library (see `genesisStates` folder).

TODO: add code example here!

The genesis block from the initialized `Blockchain` can be retrieved via the `Blockchain.genesisBlock` getter. For creating a genesis block from the params in `@ethereumjs/common`, the `createGenesisBlock(stateRoot: Buffer): Block` method can be used.

## EIP-1559 Support

This library supports the handling of `EIP-1559` blocks and transactions starting with the `v5.3.0` release.

## API

### Docs

Generated TypeDoc API [Documentation](./docs/README.md)

### BigInt Support

Starting with v6 the usage of [BN.js](https://github.com/indutny/bn.js/) for big numbers has been removed from the library and replaced with the usage of the native JS [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) data type (introduced in `ES2020`).

Please note that number-related API signatures have changed along with this version update and the minimal build target has been updated to `ES2020`.

## Developer

For debugging blockchain control flows the [debug](https://github.com/visionmedia/debug) library is used and can be activated on the CL with `DEBUG=[Logger Selection] node [Your Script to Run].js`.

The following initial logger is currently available:

| Logger              | Description                                                 |
| ------------------- | ----------------------------------------------------------- |
| `blockchain:clique` | Clique operations like updating the vote and/or signer list |

The following is an example for a logger run:

Run with the clique logger:

```shell
DEBUG=blockchain:clique ts-node test.ts
```

## EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices. If you want to join for work or carry out improvements on the libraries, please review our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html) first.

## License

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[blockchain-npm-badge]: https://img.shields.io/npm/v/@ethereumjs/blockchain.svg
[blockchain-npm-link]: https://www.npmjs.com/package/@ethereumjs/blockchain
[blockchain-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20blockchain?label=issues
[blockchain-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+blockchain"
[blockchain-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/Blockchain/badge.svg
[blockchain-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22Blockchain%22
[blockchain-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=blockchain
[blockchain-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/blockchain
