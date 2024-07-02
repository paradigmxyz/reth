# rlp

[![NPM Package][rlp-npm-badge]][rlp-npm-link]
[![GitHub Issues][rlp-issues-badge]][rlp-issues-link]
[![Actions Status][rlp-actions-badge]][rlp-actions-link]
[![Code Coverage][rlp-coverage-badge]][rlp-coverage-link]
[![Discord][discord-badge]][discord-link]

[Recursive Length Prefix](https://eth.wiki/en/fundamentals/rlp) encoding for Node.js and the browser.

## INSTALL

`npm install rlp`

install with `-g` if you want to use the CLI.

## USAGE

```typescript
import assert from 'assert'
import RLP from 'rlp'

const nestedList = [[], [[]], [[], [[]]]]
const encoded = RLP.encode(nestedList)
const decoded = RLP.decode(encoded)
assert.deepEqual(nestedList, decoded)
```

## API

`RLP.encode(plain)` - RLP encodes an `Array`, `Uint8Array` or `String` and returns a `Uint8Array`.

`RLP.decode(encoded, [stream=false])` - Decodes an RLP encoded `Uint8Array`, `Array` or `String` and returns a `Uint8Array` or `NestedUint8Array`. If `stream` is enabled, it will just decode the first rlp sequence in the Uint8Array. By default, it would throw an error if there are more bytes in Uint8Array than used by the rlp sequence.

### Buffer compatibility

If you would like to continue using Buffers like in rlp v2, you can use:

```typescript
import assert from 'assert'
import { arrToBufArr, bufArrToArr } from 'ethereumjs-util'
import RLP from 'rlp'

const bufferList = [Buffer.from('123', 'hex'), Buffer.from('456', 'hex')]
const encoded = RLP.encode(bufArrToArr(bufferList))
const encodedAsBuffer = Buffer.from(encoded)
const decoded = RLP.decode(Uint8Array.from(encodedAsBuffer)) // or RLP.decode(encoded)
const decodedAsBuffers = arrToBufArr(decoded)
assert.deepEqual(bufferList, decodedAsBuffers)
```

## CLI

`rlp encode <JSON string>`\
`rlp decode <0x-prefixed hex string>`

### Examples

- `rlp encode '5'` -> `0x05`
- `rlp encode '[5]'` -> `0xc105`
- `rlp encode '["cat", "dog"]'` -> `0xc88363617483646f67`
- `rlp decode 0xc88363617483646f67` -> `["cat","dog"]`

## TESTS

Tests use mocha.

To run tests and linting: `npm test`

To auto-fix linting problems run: `npm run lint:fix`

## CODE COVERAGE

Install dev dependencies: `npm install`

Run coverage: `npm run coverage`

The results will be at: `coverage/lcov-report/index.html`

# EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices.

If you want to join for work or do improvements on the libraries have a look at our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html).

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[rlp-npm-badge]: https://img.shields.io/npm/v/rlp.svg
[rlp-npm-link]: https://www.npmjs.com/package/rlp
[rlp-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20rlp?label=issues
[rlp-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+rlp"
[rlp-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/rlp/badge.svg
[rlp-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22rlp%22
[rlp-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=rlp
[rlp-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/rlp
