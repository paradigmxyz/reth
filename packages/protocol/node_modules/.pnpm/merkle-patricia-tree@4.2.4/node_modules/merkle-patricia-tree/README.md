# merkle-patricia-tree

[![NPM Package][trie-npm-badge]][trie-npm-link]
[![GitHub Issues][trie-issues-badge]][trie-issues-link]
[![Actions Status][trie-actions-badge]][trie-actions-link]
[![Code Coverage][trie-coverage-badge]][trie-coverage-link]
[![Discord][discord-badge]][discord-link]

This is an implementation of the modified merkle patricia tree as specified in the [Ethereum Yellow Paper](http://gavwood.com/Paper.pdf):

> The modified Merkle Patricia tree (trie) provides a persistent data structure to map between arbitrary-length binary data (byte arrays). It is defined in terms of a mutable data structure to map between 256-bit binary fragments and arbitrary-length binary data. The core of the trie, and its sole requirement in terms of the protocol specification is to provide a single 32-byte value that identifies a given set of key-value pairs.

The only backing store supported is LevelDB through the `levelup` module.

# INSTALL

`npm install merkle-patricia-tree`

# USAGE

There are 3 variants of the tree implemented in this library, namely: `BaseTrie`, `CheckpointTrie` and `SecureTrie`. `CheckpointTrie` adds checkpointing functionality to the `BaseTrie` with the methods `checkpoint`, `commit` and `revert`. `SecureTrie` extends `CheckpointTrie` and is the most suitable variant for Ethereum applications. It stores values under the `keccak256` hash of their keys.

By default, trie nodes are not deleted from the underlying DB to not corrupt older trie states (as of `v4.2.0`). If you are only interested in the latest state of a trie, you can switch to a delete behavior (e.g. if you want to save disk space) by using the `deleteFromDB` constructor option (see related release notes in the changelog for more details).

## Initialization and Basic Usage

```typescript
import level from 'level'
import { BaseTrie as Trie } from 'merkle-patricia-tree'

const db = level('./testdb')
const trie = new Trie(db)

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  const value = await trie.get(Buffer.from('test'))
  console.log(value.toString()) // 'one'
}

test()
```

## Proofs

### Merkle Proofs

The `createProof` and `verifyProof` functions allow you to verify that a certain value does or does not exist within a Merkle-Patricia trie with a given root.

#### Proof of existence

The below code demonstrates how to construct and then verify a proof that proves that the key `test` that corresponds to the value `one` does exist in the given trie, so a proof of existence.

```typescript
const trie = new Trie()

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  const proof = await Trie.createProof(trie, Buffer.from('test'))
  const value = await Trie.verifyProof(trie.root, Buffer.from('test'), proof)
  console.log(value.toString()) // 'one'
}

test()
```

#### Proof of non-existence

The below code demonstrates how to construct and then verify a proof that proves that the key `test3` does not exist in the given trie, so a proof of non-existence.

```typescript
const trie = new Trie()

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  await trie.put(Buffer.from('test2'), Buffer.from('two'))
  const proof = await Trie.createProof(trie, Buffer.from('test3'))
  const value = await Trie.verifyProof(trie.root, Buffer.from('test3'), proof)
  console.log(value.toString()) // null
}

test()
```

#### Invalid proofs

Note, if `verifyProof` detects an invalid proof, it throws an error. While contrived, the below example demonstrates the error condition that would result if a prover tampers with the data in a merkle proof.

```typescript
const trie = new Trie()

async function test() {
  await trie.put(Buffer.from('test'), Buffer.from('one'))
  await trie.put(Buffer.from('test2'), Buffer.from('two'))
  const proof = await Trie.createProof(trie, Buffer.from('test2'))
  proof[1].reverse()
  try {
    const value = await Trie.verifyProof(trie.root, Buffer.from('test2'), proof)
    console.log(value.toString()) // results in error
  } catch (err) {
    console.log(err) // Missing node in DB
  }
}

test()
```

### Range Proofs

The `Trie.verifyRangeProof()` function can be used to check whether the given leaf nodes and edge proof can prove the given trie leaves range is matched with the specific root (useful e.g. for snapsync).

## Read stream on Geth DB

```typescript
import level from 'level'
import { SecureTrie as Trie } from 'merkle-patricia-tree'

const db = level('YOUR_PATH_TO_THE_GETH_CHAIN_DB')
// Set stateRoot to block #222
const stateRoot = '0xd7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544'
// Convert the state root to a Buffer (strip the 0x prefix)
const stateRootBuffer = Buffer.from(stateRoot.slice(2), 'hex')
// Initialize trie
const trie = new Trie(db, stateRootBuffer)

trie
  .createReadStream()
  .on('data', console.log)
  .on('end', () => {
    console.log('End.')
  })
```

## Read Account State including Storage from Geth DB

```typescript
import level from 'level'
import { Account, BN, bufferToHex, rlp } from 'ethereumjs-util'
import { SecureTrie as Trie } from 'merkle-patricia-tree'

const stateRoot = 'STATE_ROOT_OF_A_BLOCK'

const db = level('YOUR_PATH_TO_THE_GETH_CHAINDATA_FOLDER')
const trie = new Trie(db, stateRoot)

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
  storageTrie.root = acc.stateRoot

  console.log('------Storage------')
  const stream = storageTrie.createReadStream()
  stream
    .on('data', (data) => {
      console.log(`key: ${bufferToHex(data.key)}`)
      console.log(`Value: ${bufferToHex(rlp.decode(data.value))}`)
    })
    .on('end', () => {
      console.log('Finished reading storage.')
    })
}

test()
```

Additional examples with detailed explanations are available [here](https://github.com/gabrocheleau/merkle-patricia-trees-examples).

# API

[Documentation](./docs/README.md)

# TESTING

`npm test`

# BENCHMARKS

There are two simple **benchmarks** in the `benchmarks` folder:

- `random.ts` runs random `PUT` operations on the tree.
- `checkpointing.ts` runs checkpoints and commits between `PUT` operations.

A third benchmark using mainnet data to simulate real load is also under consideration.

Benchmarks can be run with:

```shell
npm run benchmarks
```

To run a **profiler** on the `random.ts` benchmark and generate a flamegraph with [0x](https://github.com/davidmarkclements/0x) you can use:

```shell
npm run profiling
```

0x processes the stacks and generates a profile folder (`<pid>.0x`) containing [`flamegraph.html`](https://github.com/davidmarkclements/0x/blob/master/docs/ui.md).

# REFERENCES

- Wiki
  - [Ethereum Trie Specification](https://github.com/ethereum/wiki/wiki/Patricia-Tree)
- Blog posts
  - [Ethereum's Merkle Patricia Trees - An Interactive JavaScript Tutorial](https://rockwaterweb.com/ethereum-merkle-patricia-trees-javascript-tutorial/)
  - [Merkling in Ethereum](https://blog.ethereum.org/2015/11/15/merkling-in-ethereum/)
  - [Understanding the Ethereum Trie](https://easythereentropy.wordpress.com/2014/06/04/understanding-the-ethereum-trie/). Worth a read, but the Python libraries are outdated.
- Videos
  - [Trie and Patricia Trie Overview](https://www.youtube.com/watch?v=jXAHLqQthKw&t=26s)

# EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices.

If you want to join for work or do improvements on the libraries have a look at our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html).

# LICENSE

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[trie-npm-badge]: https://img.shields.io/npm/v/merkle-patricia-tree.svg
[trie-npm-link]: https://www.npmjs.com/package/merkle-patricia-tree
[trie-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20trie?label=issues
[trie-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+trie"
[trie-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/Trie/badge.svg
[trie-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22Trie%22
[trie-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=trie
[trie-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/trie
