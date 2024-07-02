# @ethereumjs/ethash

[![NPM Package][ethash-npm-badge]][ethash-npm-link]
[![GitHub Issues][ethash-issues-badge]][ethash-issues-link]
[![Actions Status][ethash-actions-badge]][ethash-actions-link]
[![Code Coverage][ethash-coverage-badge]][ethash-coverage-link]
[![Discord][discord-badge]][discord-link]

| [Ethash](https://github.com/ethereum/wiki/wiki/Ethash) implementation in TypeScript. |
| ------------------------------------------------------------------------------------ |

Note: this `README` reflects the state of the library from `v1.0.0` onwards. See `README` from the [standalone repository](https://github.com/ethereumjs/ethashjs) for an introduction on the last preceding release.

## Installation

To obtain the latest version, simply require the project using `npm`:

```shell
npm install @ethereumjs/ethash
```

## Usage

### PoW Validation

```typescript
import { Ethash } from '@ethereumjs/ethash'
import { Block } from '@ethereumjs/block'
import { MemoryLevel } from 'memory-level'

const cacheDB = level()

const ethash = new Ethash(cacheDB)
const validblockRlp =
  'f90667f905fba0a8d5b7a4793baaede98b5236954f634a0051842df6a252f6a80492fd888678bda01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347948888f1f195afa192cfee860698584c030f4c9db1a0f93c8db1e931daa2e22e39b5d2da6fb4074e3d544094857608536155e3521bc1a0bb7495628f9160ddbcf6354380ee32c300d594e833caec3a428041a66e7bade1a0c7778a7376099ee2e5c455791c1885b5c361b95713fddcbe32d97fd01334d296b90100000000000000000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200000000000000000008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200000000000400000000000000000000000000000000000000000000000000000008302000001832fefd882560b84559c17b9b9040001020304050607080910111213141516171819202122232410000000000000000000200000000000000000003000000000000000000040000000000000000000500000000000000000006000000000000000000070000000000000000000800000000000000000009000000000000000000010000000000000000000100000000000000000002000000000000000000030000000000000000000400000000000000000005000000000000000000060000000000000000000700000000000000000008000000000000000000090000000000000000000100000000000000000001000000000000000000020000000000000000000300000000000000000004000000000000000000050000000000000000000600000000000000000007000000000000000000080000000000000000000900000000000000000001000000000000000000010000000000000000000200000000000000000003000000000000000000040000000000000000000500000000000000000006000000000000000000070000000000000000000800000000000000000009000000000000000000010000000000000000000100000000000000000002000000000000000000030000000000000000000400000000000000000005000000000000000000060000000000000000000700000000000000000008000000000000000000090000000000000000000100000000000000000001000000000000000000020000000000000000000300000000000000000004000000000000000000050000000000000000000600000000000000000007000000000000000000080000000000000000000900000000000000000001000000000000000000010000000000000000000200000000000000000003000000000000000000040000000000000000000500000000000000000006000000000000000000070000000000000000000800000000000000000009000000000000000000010000000000000000000100000000000000000002000000000000000000030000000000000000000400000000000000000005000000000000000000060000000000000000000700000000000000000008000000000000000000090000000000000000000100000000000000000001000000000000000000020000000000000000000300000000000000000004000000000000000000050000000000000000000600000000000000000007000000000000000000080000000000000000000900000000000000000001000000000000000000010000000000000000000200000000000000000003000000000000000000040000000000000000000500000000000000000006000000000000000000070000000000000000000800000000000000000009000000000000000000010000000000000000000a09c7b47112a3afb385c12924bf6280d273c106eea7caeaf5131d8776f61056c148876ae05d46b58d1fff866f864800a82c35094095e7baea6a6c7c4c2dfeb977efac326af552d8785012a05f200801ba01d2c92cfaeb04e53acdff2b5d42005ff6aacdb0105e64eb8c30c273f445d2782a01e7d50ffce57840360c57d94977b8cdebde614da23e8d1e77dc07928763cfe21c0'

const validBlock = Block.fromRLPSerializedBlock(Buffer.from(validblockRlp, 'hex'))

const result = await ethash.verifyPOW(validBlock)
console.log(result) // => true
```

### PoW Ethash CPU Miner

There is a simple CPU miner included within `Ethash` package which can be used for testing purposes.

See the following example on how to use the new `Miner` class:

```typescript
import { Block } from '@ethereumjs/block'
import { Ethash } from '@ethereumjs/ethash'
import { Common } from '@ethereumjs/common'
import { BN } from 'ethereumjs-util'
import { MemoryLevel } from 'memory-level'

const cacheDB = new MemoryLevel()
const block = Block.fromBlockData({
  header: {
    difficulty: BigInt(100),
    number: BigInt(1),
  },
})

const e = new Ethash(cacheDB)
const miner = e.getMiner(block.header)
const solution = await miner.iterate(-1) // iterate until solution is found
```

## API

### Docs

Generated TypeDoc API [Documentation](./docs/README.md)

### BigInt Support

Starting with v2 the usage of [BN.js](https://github.com/indutny/bn.js/) for big numbers has been removed from the library and replaced with the usage of the native JS [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) data type (introduced in `ES2020`).

Please note that number-related API signatures have changed along with this version update and the minimal build target has been updated to `ES2020`.

## EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices. If you want to join for work or carry out improvements on the libraries, please review our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html) first.

# LICENSE

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[ethash-npm-badge]: https://img.shields.io/npm/v/@ethereumjs/ethash.svg
[ethash-npm-link]: https://www.npmjs.org/package/@ethereumjs/ethash
[ethash-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20ethash?label=issues
[ethash-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+ethash"
[ethash-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/Ethash/badge.svg
[ethash-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22Ethash%22
[ethash-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=ethash
[ethash-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/ethash
