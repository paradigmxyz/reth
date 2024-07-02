# @ethereumjs/trie

[![NPM Package][trie-npm-badge]][trie-npm-link]
[![GitHub Issues][trie-issues-badge]][trie-issues-link]
[![Actions Status][trie-actions-badge]][trie-actions-link]
[![Code Coverage][trie-coverage-badge]][trie-coverage-link]
[![Discord][discord-badge]][discord-link]

This is an implementation of the [Modified Merkle Patricia Trie](https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/) as specified in the [Ethereum Yellow Paper](http://gavwood.com/Paper.pdf):

> The modified Merkle Patricia tree (trie) provides a persistent data structure to map between arbitrary-length binary data (byte arrays). It is defined in terms of a mutable data structure to map between 256-bit binary fragments and arbitrary-length binary data. The core of the trie, and its sole requirement in terms of the protocol specification, is to provide a single 32-byte value that identifies a given set of key-value pairs.

## Installation

To obtain the latest version, simply require the project using `npm`:

```shell
npm install @ethereumjs/trie
```

### Upgrading

If you currently use this package in your project and plan to upgrade, please review our [upgrade guide](./UPGRADING.md) first. It will ensure you take all the necessary steps and streamline the upgrade process.

## Usage

This class implements the basic [Modified Merkle Patricia Trie](https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/) in the `Trie` base class, which you can use with the `useKeyHashing` option set to `true` to create a trie which stores values under the `keccak256` hash of its keys (this is the Trie flavor which is used in Ethereum production systems).

**Note:** Up to v4 of the Trie library the secure trie was implemented as a separate `SecureTrie` class, see the [upgrade guide](./UPGRADING.md) for more infos.

An additional `CheckpointTrie` implementation adds checkpointing functionality to `Trie` through the methods `checkpoint`, `commit` and `revert`.

It is best to select the variant that is most appropriate for your unique use case.

### Initialization and Basic Usage

```typescript
import { Trie, LevelDB } from '@ethereumjs/trie'
import { Level } from 'level'

const trie = new Trie({ db: new LevelDB(new Level('MY_TRIE_DB_LOCATION')) })

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  const value = await trie.get(Buffer.from('test'))
  console.log(value.toString()) // 'one'
}

test()
```

You can also review our [examples](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/trie/examples) for database implementations. The [level.js](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/trie/examples/level.js) example is the default implementation while [lmdb.js](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/trie/examples/lmdb.js) is an alternative implementation that uses the popular [LMDB](https://en.wikipedia.org/wiki/Lightning_Memory-Mapped_Database) as its underlying database.

> If no `db` option is provided, an in-memory database powered by [Map](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Map) will fulfill this role.

### Database

> By default the only supported database is LevelDB via the `level` module.

The 5.0.0 release introduced the `DB` interface to allow for the decoupling of the database layer from the previously tightly-coupled `LevelDB` integration. The `DB` interface defines the methods `get`, `put`, `del`, `batch` and `copy` that a concrete implementation of the `DB` interface will need to implement. The default implementation of the `DB` interface is now an in-memory storage based on the native [Map](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Map) and functions identically to pre-5.0.0 releases.

The base trie implementation (`Trie`) as well as all subclass implementations (`CheckpointTrie` and `SecureTrie`) accept any database implementation that adheres to the `DB` interface as the `db` option. It is possible to use alternative implementations like [LevelDB](#leveldb) if you wish to.

#### Node Deletion

By default, the deletion of trie nodes from the underlying database does not occur in order to avoid corrupting older trie states (as of `v4.2.0`). Should you only wish to work with the latest state of a trie, you can switch to a delete behavior (for example, if you wish to save disk space) by using the `deleteFromDB` constructor option (see related release notes in the changelog for further details).

#### Persistence

You can enable persistence by setting the `useRootPersistence` option to `true` when constructing a trie through the `Trie.create` function. As such, this value is preserved when creating copies of the trie and is incapable of being modified once a trie is instantiated.

```typescript
import { Trie, LevelDB } from '@ethereumjs/trie'
import { Level } from 'level'

const trie = await Trie.create({
  db: new LevelDB(new Level('MY_TRIE_DB_LOCATION')),
  useRootPersistence: true,
})
```

The `Trie.create` function is asynchronous and will read the root from your database before returning the trie instance. If you don't have the need for automatic restoration of the root then you can use the `new Trie` constructor with the same options and get persistence without the automatic restoration.

#### LevelDB

If you wish to continue to rely on `LevelDB` for all operations then you should create a file with the [following implementation from our recipes](./recipes//level.ts) in your project. It is then possible to use the `LevelDB` implementation as follows:

```typescript
import { Trie } from '@ethereumjs/trie'
import { Level } from 'level'

import { LevelDB } from './your-level-implementation'

const trie = new Trie({ db: new LevelDB(new Level('MY_TRIE_DB_LOCATION')) })
```

## Proofs

### Merkle Proofs

The `createProof` and `verifyProof` functions allow you to verify that a certain value does or does not exist within a Merkle Patricia Tree with a given root.

#### Proof-of-Inclusion

The following code demonstrates how to construct and subsequently verify a proof that confirms the existence of the key `test` (which corresponds with the value `one`) within the given trie. This is also known as inclusion, hence the name 'Proof-of-Inclusion.'

```typescript
const trie = new Trie()

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  const proof = await trie.createProof(Buffer.from('test'))
  const value = await trie.verifyProof(trie.root(), Buffer.from('test'), proof)
  console.log(value.toString()) // 'one'
}

test()
```

#### Proof-of-Exclusion

The following code demonstrates how to construct and subsequently verify a proof that confirms that the key `test3` does not exist within the given trie. This is also known as exclusion, hence the name 'Proof-of-Exclusion.'

```typescript
const trie = new Trie()

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  await trie.put(Buffer.from('test2'), Buffer.from('two'))
  const proof = await trie.createProof(Buffer.from('test3'))
  const value = await trie.verifyProof(trie.root(), Buffer.from('test3'), proof)
  console.log(value.toString()) // null
}

test()
```

#### Invalid Proofs

If `verifyProof` detects an invalid proof, it will throw an error. While contrived, the below example illustrates the resulting error condition in the event a prover tampers with the data in a merkle proof.

```typescript
const trie = new Trie()

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  await trie.put(Buffer.from('test2'), Buffer.from('two'))
  const proof = await trie.createProof(Buffer.from('test2'))
  proof[1].reverse()
  try {
    const value = await trie.verifyProof(trie.root(), Buffer.from('test2'), proof)
    console.log(value.toString()) // results in error
  } catch (err) {
    console.log(err) // Missing node in DB
  }
}

test()
```

### Range Proofs

You may use the `Trie.verifyRangeProof()` function to confirm if the given leaf nodes and edge proof possess the capacity to prove that the given trie leaves' range matches the specific root (which is useful for snap sync, for instance).

## Read Stream on Geth DB

```typescript
import { Level } from 'level'
import { LevelDB, Trie } from '@ethereumjs/trie'

// Set stateRoot to block #222
const stateRoot = '0xd7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544'
// Convert the state root to a Buffer (strip the 0x prefix)
const stateRootBuffer = Buffer.from(stateRoot.slice(2), 'hex')
// Initialize trie
const trie = new Trie({
  db: new LevelDB(new Level('YOUR_PATH_TO_THE_GETH_CHAIN_DB')),
  root: stateRootBuffer,
  useKeyHashing: true,
})

trie
  .createReadStream()
  .on('data', console.log)
  .on('end', () => console.log('End.'))
```

## Read Account State Including Storage From Geth DB

```typescript
import { Level } from 'level'
import { Trie, LevelDB } from '@ethereumjs/trie'
import { Account, bufferToHex } from '@ethereumjs/util'
import { RLP } from '@ethereumjs/rlp'

const stateRoot = 'STATE_ROOT_OF_A_BLOCK'

const trie = new Trie({
  db: new LevelDB(new Level('YOUR_PATH_TO_THE_GETH_CHAINDATA_FOLDER')),
  root: stateRoot
  useKeyHashing: true,
})

const address = 'AN_ETHEREUM_ACCOUNT_ADDRESS'

async function test() {
  const data = await trie.get(address)
  const acc = Account.fromAccountData(data)

  console.log('-------State-------')
  console.log(`nonce: ${acc.nonce}`)
  console.log(`balance in wei: ${acc.balance}`)
  console.log(`storageRoot: ${bufferToHex(acc.stateRoot)}`)
  console.log(`codeHash: ${bufferToHex(acc.codeHash)}`)

  const storageTrie = trie.copy()
  storageTrie.root(acc.stateRoot)

  console.log('------Storage------')
  const stream = storageTrie.createReadStream()
  stream
    .on('data', (data) => {
      console.log(`key: ${bufferToHex(data.key)}`)
      console.log(`Value: ${bufferToHex(Buffer.from(RLP.decode(data.value)))}`)
    })
    .on('end', () => console.log('Finished reading storage.'))
}

test()
```

You can find additional examples complete with detailed explanations [here](./examples/README.md).

## API

### Docs

Generated TypeDoc API [Documentation](./docs/README.md)

### BigInt Support

With the 5.0.0 release, [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) takes the place of [BN.js](https://github.com/indutny/bn.js/).

BigInt is a primitive that is used to represent and manipulate primitive `bigint` values that the number primitive is incapable of representing as a result of their magnitude. `ES2020` saw the introduction of this particular feature. Note that this version update resulted in the altering of number-related API signatures and that the minimal build target is now set to `ES2020`.

## Testing

You may run tests for browsers and node.js using:

```shell
npm run test
```

You may run tests for browsers using:

```shell
npm run test:browser
```

> Note that this requires an installation of [Mozilla Firefox](https://www.mozilla.org/en-US/firefox/new/), otherwise the tests will fail.

You may run tests for node.js using:

```shell
npm run test:node
```

## Benchmarking

You will find two simple **benchmarks** in the `benchmarks` folder:

- `random.ts` runs random `PUT` operations on the tree, and
- `checkpointing.ts` runs checkpoints and commits between `PUT` operations

A third benchmark using mainnet data to simulate real load is also being considered.

You may run benchmarks using:

```shell
npm run benchmarks
```

To run a **profiler** on the `random.ts` benchmark and generate a flamegraph with [0x](https://github.com/davidmarkclements/0x), you may use:

```shell
npm run profiling
```

0x processes the stacks and generates a profile folder (`<pid>.0x`) containing [`flamegraph.html`](https://github.com/davidmarkclements/0x/blob/master/docs/ui.md).

## References

- Wiki
  - [Ethereum Trie Specification](https://github.com/ethereum/wiki/wiki/Patricia-Tree)
- Blog posts
  - [Ethereum's Merkle Patricia Trees - An Interactive JavaScript Tutorial](https://rockwaterweb.com/ethereum-merkle-patricia-trees-javascript-tutorial/)
  - [Merkling in Ethereum](https://blog.ethereum.org/2015/11/15/merkling-in-ethereum/)
  - [Understanding the Ethereum Trie](https://easythereentropy.wordpress.com/2014/06/04/understanding-the-ethereum-trie/) (This is worth reading, but mind the outdated Python libraries)
- Videos
  - [Trie and Patricia Trie Overview](https://www.youtube.com/watch?v=jXAHLqQthKw&t=26s)

## EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices. If you want to join for work or carry out improvements on the libraries, please review our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html) first.

## License

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[trie-npm-badge]: https://img.shields.io/npm/v/@ethereumjs/trie.svg
[trie-npm-link]: https://www.npmjs.com/package/@ethereumjs/trie
[trie-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20trie?label=issues
[trie-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+trie"
[trie-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/Trie/badge.svg
[trie-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22Trie%22
[trie-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=trie
[trie-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/trie
