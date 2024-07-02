# @ethereumjs/evm

[![NPM Package][evm-npm-badge]][evm-npm-link]
[![GitHub Issues][evm-issues-badge]][evm-issues-link]
[![Actions Status][evm-actions-badge]][evm-actions-link]
[![Code Coverage][evm-coverage-badge]][evm-coverage-link]
[![Discord][discord-badge]][discord-link]

| TypeScript implementation of the Ethereum EVM. |
| ---------------------------------------------- |

## Installation

To obtain the latest version, simply require the project using `npm`:

```shell
npm install @ethereumjs/evm
```

This package provides the core Ethereum Virtual Machine (EVM) implementation which is capable of executing EVM-compatible bytecode. The package has been extracted from the [@ethereumjs/vm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/vm) package along the VM `v6` release.

Note that this package atm cannot be run in a standalone mode but needs to be executed via the `VM` package which provides an outer Ethereum `mainnet` compatible execution context. Standalone functionality will be added along a future non-breaking release.

## Usage

```typescript
import { Blockchain } from '@ethereumjs/blockchain'
import { Chain, Common, Hardfork } from '@ethereumjs/common'
import { EEI } from '@ethereumjs/vm'
import { EVM } from '@ethereumjs/evm'
import { DefaultStateManager } from '@ethereumjs/statemanager'

// Note: in a future release there will be an EEI default implementation
// which will ease standalone initialization
const main = async () => {
  const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.London })
  const stateManager = new DefaultStateManager()
  const blockchain = await Blockchain.create()
  const eei = new EEI(stateManager, common, blockchain)

  const evm = new EVM({
    common,
    eei,
  })

  const STOP = '00'
  const ADD = '01'
  const PUSH1 = '60'

  // Note that numbers added are hex values, so '20' would be '32' as decimal e.g.
  const code = [PUSH1, '03', PUSH1, '05', ADD, STOP]

  evm.events.on('step', function (data) {
    // Note that data.stack is not immutable, i.e. it is a reference to the vm's internal stack object
    console.log(`Opcode: ${data.opcode.name}\tStack: ${data.stack}`)
  })

  evm
    .runCode({
      code: Buffer.from(code.join(''), 'hex'),
      gasLimit: BigInt(0xffff),
    })
    .then((results) => {
      console.log(`Returned: ${results.returnValue.toString('hex')}`)
      console.log(`gasUsed: ${results.executionGasUsed.toString()}`)
    })
    .catch(console.error)
}

void main()
```

### Example

This projects contain the following examples:

1. [./examples/decode-opcodes](./examples/decode-opcodes.ts): Decodes a binary EVM program into its opcodes.
1. [./examples/run-code-browser](./examples/run-code-browser.js): Show how to use this library in a browser.

All of the examples have their own `README.md` explaining how to run them.

## API

### Docs

For documentation on `EVM` instantiation, exposed API and emitted `events` see generated [API docs](./docs/README.md).

### BigInt Support

Starting with v1 the usage of [BN.js](https://github.com/indutny/bn.js/) for big numbers has been removed from the library and replaced with the usage of the native JS [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) data type (introduced in `ES2020`).

Please note that number-related API signatures have changed along with this version update and the minimal build target has been updated to `ES2020`.

## Architecture

### VM/EVM Relation

This package contains the inner Ethereum Virtual Machine core functionality which was included in the [@ethereumjs/vm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/vm) package up till v5 and has been extracted along the v6 release.

This will make it easier to customize the inner EVM, which can now be passed as an optional argument to the outer `VM` instance.

At the moment the `EVM` package can not be run standalone and it is therefore recommended for most use cases to rather use the `VM` package and access `EVM` functionality through the `vm.evm` property.

### Execution Environment (EEI) and State

For the EVM to properly work it needs access to a respective execution environment (to e.g. request on information like block hashes) as well as the connection to an outer account and contract state.

To ensure a unified interface the `EVM` provides a TypeScript `EEI` interface providing which includes the necessary function signatures for access to environmental parameters as well as the VM state.

The [@ethereumjs/vm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/vm) provides a concrete implementation of this interface which can be used to instantiate the `EVM` within an Ethereum `mainnet` compatible execution context.

## Browser

To build the EVM for standalone use in the browser, see: [Running the EVM in a browser](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm/examples/run-code-browser.js).

## Setup

### Hardfork Support

The EthereumJS EVM implements all hardforks from `Frontier` (`chainstart`) up to the latest active mainnet hardfork.

Currently the following hardfork rules are supported:

- `chainstart` (a.k.a. Frontier)
- `homestead`
- `tangerineWhistle`
- `spuriousDragon`
- `byzantium`
- `constantinople`
- `petersburg`
- `istanbul`
- `muirGlacier` (only `mainnet` and `ropsten`)
- `berlin` (`v5.2.0`+)
- `london` (`v5.4.0`+)
- `arrowGlacier` (only `mainnet`) (`v5.6.0`+)
- `merge` (only `goerli`, `ropsten` and soon `mainnet`)

Default: `merge` (taken from `Common.DEFAULT_HARDFORK`)

A specific hardfork EVM ruleset can be activated by passing in the hardfork
along the `Common` instance to the outer `@ethereumjs/vm` instance.

### EIP Support

It is possible to individually activate EIP support in the EVM by instantiate the `Common` instance passed to the outer VM with the respective EIPs, e.g.:

```typescript
import { Chain, Common } from '@ethereumjs/common'
import { VM } from '@ethereumjs/vm'

const common = new Common({ chain: Chain.Mainnet, eips: [2537] })
const vm = new VM({ common })
```

Currently supported EIPs:

- [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) - Fee Market (`london` EIP)
- [EIP-2315](https://eips.ethereum.org/EIPS/eip-2315) - Simple subroutines (`experimental`)
- [EIP-2537](https://eips.ethereum.org/EIPS/eip-2537) - BLS precompiles (`experimental`)
- [EIP-2565](https://eips.ethereum.org/EIPS/eip-2565) - ModExp gas cost (`berlin` EIP)
- [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718) - Typed transactions (`berlin` EIP)
- [EIP-2929](https://eips.ethereum.org/EIPS/eip-2929) - Gas cost increases for state access opcodes (`berlin` EIP)
- [EIP-2930](https://eips.ethereum.org/EIPS/eip-2930) - Optional Access Lists Typed Transactions (`berlin` EIP)
- [EIP-3198](https://eips.ethereum.org/EIPS/eip-3198) - BASEFEE opcode (`london` EIP)
- [EIP-3529](https://eips.ethereum.org/EIPS/eip-3529) - Reduction in refunds (`london` EIP)
- [EIP-3540](https://eips.ethereum.org/EIPS/eip-3541) - EVM Object Format (EOF) v1 (`experimental`)
- [EIP-3541](https://eips.ethereum.org/EIPS/eip-3541) - Reject new contracts starting with the 0xEF byte (`london` EIP)
- [EIP-3670](https://eips.ethereum.org/EIPS/eip-3670) - EOF - Code Validation (`experimental`)
- [EIP-3855](https://eips.ethereum.org/EIPS/eip-3855) - PUSH0 instruction (`experimental`)
- [EIP-3860](https://eips.ethereum.org/EIPS/eip-3860) - Limit and meter initcode (`experimental`)
- [EIP-4399](https://eips.ethereum.org/EIPS/eip-4399) - Supplant DIFFICULTY opcode with PREVRANDAO (Merge) (`experimental`)

### Tracing Events

Our `TypeScript` EVM is implemented as an [AsyncEventEmitter](https://github.com/ahultgren/async-eventemitter) and events are submitted along major execution steps which you can listen to.

You can subscribe to the following events:

- `beforeMessage`: Emits a `Message` right after running it.
- `afterMessage`: Emits an `EVMResult` right after running a message.
- `step`: Emits an `InterpreterStep` right before running an EVM step.
- `newContract`: Emits a `NewContractEvent` right before creating a contract. This event contains the deployment code, not the deployed code, as the creation message may not return such a code.

An example for the `step` event can be found in the initial usage example in this `README`.

#### Asynchronous event handlers

You can perform asynchronous operations from within an event handler
and prevent the EVM to keep running until they finish.

In order to do that, your event handler has to accept two arguments.
The first one will be the event object, and the second one a function.
The EVM won't continue until you call this function.

If an exception is passed to that function, or thrown from within the
handler or a function called by it, the exception will bubble into the
EVM and interrupt it, possibly corrupting its state. It's strongly
recommended not to do that.

#### Synchronous event handlers

If you want to perform synchronous operations, you don't need
to receive a function as the handler's second argument, nor call it.

Note that if your event handler receives multiple arguments, the second
one will be the continuation function, and it must be called.

If an exception is thrown from withing the handler or a function called
by it, the exception will bubble into the EVM and interrupt it, possibly
corrupting its state. It's strongly recommended not to throw from withing
event handlers.

## Understanding the EVM

If you want to understand your EVM runs we have added a hierarchically structured list of debug loggers for your convenience which can be activated in arbitrary combinations. We also use these loggers internally for development and testing. These loggers use the [debug](https://github.com/visionmedia/debug) library and can be activated on the CL with `DEBUG=[Logger Selection] node [Your Script to Run].js` and produce output like the following:

![EthereumJS EVM Debug Logger](./debug.png?raw=true)

The following loggers are currently available:

| Logger                             | Description                                         |
| ---------------------------------- | --------------------------------------------------- |
| `evm`                              |  EVM control flow, CALL or CREATE message execution |
| `evm:gas`                          |  EVM gas logger                                     |
| `evm:eei:gas`                      |  EEI gas logger                                     |
| `evm:ops`                          |  Opcode traces                                      |
| `evm:ops:[Lower-case opcode name]` | Traces on a specific opcode                         |

Here are some examples for useful logger combinations.

Run one specific logger:

```shell
DEBUG=evm ts-node test.ts
```

Run all loggers currently available:

```shell
DEBUG=evm:*,evm:*:* ts-node test.ts
```

Run only the gas loggers:

```shell
DEBUG=evm:*:gas ts-node test.ts
```

Excluding the ops logger:

```shell
DEBUG=evm:*,evm:*:*,-evm:ops ts-node test.ts
```

Run some specific loggers including a logger specifically logging the `SSTORE` executions from the EVM (this is from the screenshot above):

```shell
DEBUG=evm,evm:ops:sstore,evm:*:gas ts-node test.ts
```

### Internal Structure

The EVM processes state changes at many levels.

- **runCall**
  - checkpoint state
  - transfer value
  - load code
  - runCode
  - materialize created contracts
  - revert or commit checkpoint
- **runCode**
  - iterate over code
  - run op codes
  - track gas usage
- **OpFns**
  - run individual op code
  - modify stack
  - modify memory
  - calculate fee

The opFns for `CREATE`, `CALL`, and `CALLCODE` call back up to `runCall`.

TODO: this section likely needs an update.

## Development

See [@ethereumjs/vm](https://github.com/ethereumjs/ethereumjs-monorepo/tree/master/packages/vm) README.

## EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices. If you want to join for work or carry out improvements on the libraries, please review our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html) first.

## License

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)

[discord-badge]: https://img.shields.io/static/v1?logo=discord&label=discord&message=Join&color=blue
[discord-link]: https://discord.gg/TNwARpR
[evm-npm-badge]: https://img.shields.io/npm/v/@ethereumjs/evm.svg
[evm-npm-link]: https://www.npmjs.com/package/@ethereumjs/evm
[evm-issues-badge]: https://img.shields.io/github/issues/ethereumjs/ethereumjs-monorepo/package:%20evm?label=issues
[evm-issues-link]: https://github.com/ethereumjs/ethereumjs-monorepo/issues?q=is%3Aopen+is%3Aissue+label%3A"package%3A+evm"
[evm-actions-badge]: https://github.com/ethereumjs/ethereumjs-monorepo/workflows/EVM/badge.svg
[evm-actions-link]: https://github.com/ethereumjs/ethereumjs-monorepo/actions?query=workflow%3A%22EVM%22
[evm-coverage-badge]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/branch/master/graph/badge.svg?flag=evm
[evm-coverage-link]: https://codecov.io/gh/ethereumjs/ethereumjs-monorepo/tree/master/packages/evm
