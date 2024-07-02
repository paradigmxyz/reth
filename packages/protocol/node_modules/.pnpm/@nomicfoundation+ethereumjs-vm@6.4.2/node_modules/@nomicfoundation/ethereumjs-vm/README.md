# @ethereumjs/vm

[![NPM Package][vm-npm-badge]][vm-npm-link]
[![GitHub Issues][vm-issues-badge]][vm-issues-link]
[![Actions Status][vm-actions-badge]][vm-actions-link]
[![Code Coverage][vm-coverage-badge]][vm-coverage-link]
[![Discord][discord-badge]][discord-link]

| Execution Context for the Ethereum EVM Implementation. |
| ------------------------------------------------------ |

This package provides an Ethereum `mainnet` compatible execution context for the
[@ethereumjs/evm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm)
EVM implementation.

Note that up till `v5` this package also was the bundled package for the EVM implementation itself.

## Installation

To obtain the latest version, simply require the project using `npm`:

```shell
npm install @ethereumjs/vm
```

## Usage

```typescript
import { Address } from '@ethereumjs/util'
import { Chain, Common, Hardfork } from '@ethereumjs/common'
import { Transaction } from '@ethereumjs/tx'
import { VM } from '@ethereumjs/vm'

const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Berlin })
const vm = await VM.create({ common })

const tx = Transaction.fromTxData({
  gasLimit: BigInt(21000),
  value: BigInt(1),
  to: Address.zero(),
  v: BigInt(37),
  r: BigInt('62886504200765677832366398998081608852310526822767264927793100349258111544447'),
  s: BigInt('21948396863567062449199529794141973192314514851405455194940751428901681436138'),
})
await vm.runTx({ tx, skipBalance: true })
```

## Example

This projects contain the following examples:

1. [./examples/run-blockchain](./examples/run-blockchain.ts): Loads tests data, including accounts and blocks, and runs all of them in the VM.
1. [./examples/run-solidity-contract](./examples/run-solidity-contract.ts): Compiles a Solidity contract, and calls constant and non-constant functions.

All of the examples have their own `README.md` explaining how to run them.

# API

### Docs

For documentation on `VM` instantiation, exposed API and emitted `events` see generated [API docs](./docs/README.md).

### BigInt Support

Starting with v6 the usage of [BN.js](https://github.com/indutny/bn.js/) for big numbers has been removed from the library and replaced with the usage of the native JS [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) data type (introduced in `ES2020`).

Please note that number-related API signatures have changed along with this version update and the minimal build target has been updated to `ES2020`.

## Architecture

### VM/EVM Relation

Starting with the `VM` v6 version the inner Ethereum Virtual Machine core previously included in this library has been extracted to an own package [@ethereumjs/evm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm).

It is still possible to access all `EVM` functionality through the `evm` property of the initialized `vm` object, e.g.:

```typescript
vm.evm.runCode() // or
vm.evm.on('step', function (data) {
  console.log(`Opcode: ${data.opcode.name}\tStack: ${data.stack}`)
})
```

Note that it now also get's possible to pass in an own or customized `EVM` instance by using the optional `evm` constructor option.

### Execution Environment (EEI) and State

This package provides a concrete implementation of the [@ethereumjs/evm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm) EEI interface to instantiate a VM/EVM combination with an Ethereum `mainnet` compatible execution context.

With `VM` v6 the previously included `StateManager` has been extracted to its own package [@ethereumjs/statemanager](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/statemanger). The `StateManager` package provides a unified state interface and it is now also possible to provide a modified or custom `StateManager` to the VM via the optional `stateManager` constructor option.

## Setup

### Chain Support

Starting with `v5.1.0` the VM supports running both `Ethash/PoW` and `Clique/PoA` blocks and transactions. Clique support has been added along the work on PR [#1032](https://github.com/ethereumjs/ethereumjs-monorepo/pull/1032) and follow-up PRs and (block) validation checks and the switch of the execution context now happens correctly.

### Ethash/PoW Chains

`@ethereumjs/blockchain` validates the PoW algorithm with `@ethereumjs/ethash` and validates blocks' difficulty to match their canonical difficulty.

### Clique/PoA Chains

The following is a simple example for a block run on `Goerli`:

```typescript
import { VM } from '@ethereumjs/vm'
import { Chain, Common } from '@ethereumjs/common'

const common = new Common({ chain: Chain.Goerli })
const hardforkByBlockNumber = true
const vm = new VM({ common, hardforkByBlockNumber })

const serialized = Buffer.from('f901f7a06bfee7294bf4457...', 'hex')
const block = Block.fromRLPSerializedBlock(serialized, { hardforkByBlockNumber })
const result = await vm.runBlock(block)
```

### Hardfork Support

For hardfork support see the [Hardfork Support](../evm#hardfork-support) section from the underlying `@ethereumjs/evm` instance.

An explicit HF in the `VM` - which is then passed on to the inner `EVM` - can be set with:

```typescript
import { Chain, Common, Hardfork } from '@ethereumjs/common'
import { VM } from '@ethereumjs/vm'

const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Berlin })
const vm = new VM({ common })
```

### Custom genesis state support

Genesis state code logic has been reworked substantially along the v6 breaking releases and a lot of the genesis state code moved from both the `@ethereumjs/common` and `@ethereumjs/block` libraries to the `@ethereumjs/blockchain` library, see PR [#1916](https://github.com/ethereumjs/ethereumjs-monorepo/pull/1916) for an overview on the broad set of changes.

For initializing a custom genesis state you can now use the `genesisState` constructor option in the `Blockchain` library in a similar way this had been done in the `Common` library before.

If you want to create a new instance of the VM and add your own genesis state, you can do it by passing a `Blockchain` instance with custom genesis state set with the `genesisState` constructor option and passing the flag `activateGenesisState` in `VMOpts`.

```typescript
import { Common } from '@ethereumjs/common'
import { VM } from '@ethereumjs/vm'
import myCustomChain1 from '[PATH_TO_MY_CHAINS]/myCustomChain1.json'
import chain1GenesisState from '[PATH_TO_GENESIS_STATES]/chain1GenesisState.json'

const common = new Common({
  // TODO: complete example
})
const blockchain = await Blockchain.create({
  // TODO: complete example
})
const vm = await VM.create({ common, activateGenesisState: true })
```

Genesis state can be configured to contain both EOAs as well as (system) contracts with initial storage values set.

### EIP Support

It is possible to individually activate EIP support in the VM by instantiate the `Common` instance passed
with the respective EIPs, e.g.:

```typescript
import { Chain, Common } from '@ethereumjs/common'
import { VM } from '@ethereumjs/vm'

const common = new Common({ chain: Chain.Mainnet, eips: [2537] })
const vm = new VM({ common })
```

For a list with supported EIPs see the [@ethereumjs/evm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm) documentation.

### Tracing Events

Our `TypeScript` VM is implemented as an [AsyncEventEmitter](https://github.com/ahultgren/async-eventemitter) and events are submitted along major execution steps which you can listen to.

You can subscribe to the following events:

- `beforeBlock`: Emits a `Block` right before running it.
- `afterBlock`: Emits `AfterBlockEvent` right after running a block.
- `beforeTx`: Emits a `Transaction` right before running it.
- `afterTx`: Emits a `AfterTxEvent` right after running a transaction.

Please note that there are additional EVM-specific events in the [@ethereumjs/evm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm) package.

#### Asynchronous event handlers

You can perform asynchronous operations from within an event handler
and prevent the VM to keep running until they finish.

In order to do that, your event handler has to accept two arguments.
The first one will be the event object, and the second one a function.
The VM won't continue until you call this function.

If an exception is passed to that function, or thrown from within the
handler or a function called by it, the exception will bubble into the
VM and interrupt it, possibly corrupting its state. It's strongly
recommended not to do that.

#### Synchronous event handlers

If you want to perform synchronous operations, you don't need
to receive a function as the handler's second argument, nor call it.

Note that if your event handler receives multiple arguments, the second
one will be the continuation function, and it must be called.

If an exception is thrown from withing the handler or a function called
by it, the exception will bubble into the VM and interrupt it, possibly
corrupting its state. It's strongly recommended not to throw from withing
event handlers.

## Understanding the VM

If you want to understand your VM runs we have added a hierarchically structured list of debug loggers for your convenience which can be activated in arbitrary combinations. We also use these loggers internally for development and testing. These loggers use the [debug](https://github.com/visionmedia/debug) library and can be activated on the CL with `DEBUG=[Logger Selection] node [Your Script to Run].js` and produce output like the following:

![EthereumJS VM Debug Logger](./debug.png?raw=true)

The following loggers are currently available:

| Logger      | Description                                                        |
| ----------- | ------------------------------------------------------------------ |
| `vm:block`  | Block operations (run txs, generating receipts, block rewards,...) |
| `vm:tx`     |  Transaction operations (account updates, checkpointing,...)       |
| `vm:tx:gas` |  Transaction gas logger                                            |
| `vm:state`  | StateManager logger                                                |

Note that there are additional EVM-specific loggers in the [@ethereumjs/evm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm) package.

Here are some examples for useful logger combinations.

Run one specific logger:

```shell
DEBUG=vm:tx ts-node test.ts
```

Run all loggers currently available:

```shell
DEBUG=vm:*,vm:*:* ts-node test.ts
```

Run only the gas loggers:

```shell
DEBUG=vm:*:gas ts-node test.ts
```

Excluding the state logger:

```shell
DEBUG=vm:*,vm:*:*,-vm:state ts-node test.ts
```

Run some specific loggers including a logger specifically logging the `SSTORE` executions from the VM (this is from the screenshot above):

```shell
DEBUG=vm:tx,vm:evm,vm:ops:sstore,vm:*:gas ts-node test.ts
```

## Internal Structure

The VM processes state changes at many levels.

- **runBlockchain**
  - for every block, runBlock
- **runBlock**
  - for every tx, runTx
  - pay miner and uncles
- **runTx**
  - check sender balance
  - check sender nonce
  - runCall
  - transfer gas charges

TODO: this section likely needs an update.

## Development

Developer documentation - currently mainly with information on testing and debugging - can be found [here](./DEVELOPER.md).

## EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices. If you want to join for work or carry out improvements on the libraries, please review our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html) first.

## License

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[vm-npm-badge]: https://img.shields.io/npm/v/@ethereumjs/vm.svg
[vm-npm-link]: https://www.npmjs.com/package/@ethereumjs/vm
[vm-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20vm?label=issues
[vm-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+vm"
[vm-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/VM/badge.svg
[vm-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22VM%22
[vm-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=vm
[vm-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/vm
