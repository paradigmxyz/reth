# SYNOPSIS

[![NPM Package](https://img.shields.io/npm/v/ethereumjs-vm.svg?style=flat-square)](https://www.npmjs.org/package/ethereumjs-vm)
[![Actions Status](https://github.com/ethereumjs/ethereumjs-vm/workflows/vm-test/badge.svg)](https://github.com/ethereumjs/ethereumjs-vm/actions)
[![Code Coverage](https://codecov.io/gh/ethereumjs/ethereumjs-vm/branch/master/graph/badge.svg)](https://codecov.io/gh/ethereumjs/ethereumjs-vm)
[![Gitter](https://img.shields.io/gitter/room/ethereum/ethereumjs.svg?style=flat-square)](https://gitter.im/ethereum/ethereumjs)

[![js-standard-style](https://cdn.rawgit.com/feross/standard/master/badge.svg)](https://github.com/feross/standard)

Implements Ethereum's VM in Javascript.

#### Fork Support

The VM currently supports the following hardfork rules:

- `Byzantium`
- `Constantinople`
- `Petersburg` (default)
- `Istanbul`
- `MuirGlacier` (only `mainnet` and `ropsten`)

If you are still looking for a [Spurious Dragon](https://eips.ethereum.org/EIPS/eip-607) compatible version of this library install the latest of the `2.2.x` series (see [Changelog](./CHANGELOG.md)).

##### MuirGlacier Hardfork Support

An Ethereum test suite compliant `MuirGlacier` HF implementation is available
since the `v4.1.3` VM release. You can activate a `MuirGlacier` VM by using the
`muirGlacier` `hardfork` option flag.

**Note:** The original `v4.1.2` release contains a critical bug preventing the
`MuirGlacier` VM to work properly and there is the need to update.

##### Istanbul Harfork Support

An Ethereum test suite compliant `Istanbul` HF implementation is available
since the `v4.1.1` VM release. You can activate an `Istanbul` VM by using the
`istanbul` `hardfork` option flag.

Supported `Istanbul` EIPs:

- [EIP-152](https://eips.ethereum.org/EIPS/eip-152): Blake 2b `F` precompile,
  PR [#584](https://github.com/ethereumjs/ethereumjs-vm/pull/584)
- [EIP-1108](https://eips.ethereum.org/EIPS/eip-1108): Reduce `alt_bn128`
  precompile gas costs,  
  PR [#540](https://github.com/ethereumjs/ethereumjs-vm/pull/540)
  (already released in `v4.0.0`)
- [EIP-1344](https://eips.ethereum.org/EIPS/eip-1344): Add ChainID Opcode,
  PR [#572](https://github.com/ethereumjs/ethereumjs-vm/pull/572)
- [EIP-1884](https://eips.ethereum.org/EIPS/eip-1884): Trie-size-dependent
  Opcode Repricing,
  PR [#581](https://github.com/ethereumjs/ethereumjs-vm/pull/581)
- [EIP-2200](https://eips.ethereum.org/EIPS/eip-2200): Rebalance net-metered
  SSTORE gas costs,
  PR [#590](https://github.com/ethereumjs/ethereumjs-vm/pull/590)

# INSTALL

`npm install ethereumjs-vm`

# USAGE

```javascript
const BN = require('bn.js')
var VM = require('ethereumjs-vm').default

// Create a new VM instance
// For explicity setting the HF use e.g. `new VM({ hardfork: 'petersburg' })`
const vm = new VM()

const STOP = '00'
const ADD = '01'
const PUSH1 = '60'

// Note that numbers added are hex values, so '20' would be '32' as decimal e.g.
const code = [PUSH1, '03', PUSH1, '05', ADD, STOP]

vm.on('step', function(data) {
  console.log(`Opcode: ${data.opcode.name}\tStack: ${data.stack}`)
})

vm.runCode({
  code: Buffer.from(code.join(''), 'hex'),
  gasLimit: new BN(0xffff),
})
  .then(results => {
    console.log('Returned : ' + results.returnValue.toString('hex'))
    console.log('gasUsed  : ' + results.gasUsed.toString())
  })
  .catch(err => console.log('Error    : ' + err))
```

## Example

This projects contain the following examples:

1. [./examples/run-blockchain](./examples/run-blockchain): Loads tests data, including accounts and blocks, and runs all of them in the VM.
1. [./examples/run-code-browser](./examples/run-code-browser): Show how to use this library in a browser.
1. [./examples/run-solidity-contract](./examples/run-solidity-contract): Compiles a Solidity contract, and calls constant and non-constant functions.
1. [./examples/run-transactions-complete](./examples/run-transactions-complete): Runs a contract-deployment transaction and then calls one of its functions.
1. [./examples/decode-opcodes](./examples/decode-opcodes): Decodes a binary EVM program into its opcodes.

All of the examples have their own `README.md` explaining how to run them.

# BROWSER

To build the VM for standalone use in the browser, see: [Running the VM in a browser](https://github.com/ethereumjs/ethereumjs-vm/tree/master/examples/run-code-browser).

# API

## VM

For documentation on `VM` instantiation, exposed API and emitted `events` see generated [API docs](./docs/README.md).

## StateManger

The API for the `StateManager` is currently in `Beta`, separate documentation can be found [here](./docs/classes/statemanager.md), see also [release notes](https://github.com/ethereumjs/ethereumjs-vm/releases/tag/v2.5.0) from the `v2.5.0` VM release for details on the `StateManager` rewrite.

# Internal Structure

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

## VM's tracing events

You can subscribe to the following events of the VM:

- `beforeBlock`: Emits a `Block` right before running it.
- `afterBlock`: Emits `RunBlockResult` right after running a block.
- `beforeTx`: Emits a `Transaction` right before running it.
- `afterTx`: Emits a `RunTxResult` right after running a transaction.
- `beforeMessage`: Emits a `Message` right after running it.
- `afterMessage`: Emits an `EVMResult` right after running a message.
- `step`: Emits an `InterpreterStep` right before running an EVM step.
- `newContract`: Emits a `NewContractEvent` right before creating a contract. This event contains the deployment code, not the deployed code, as the creation message may not return such a code.

### Asynchronous event handlers

You can perform asynchronous operations from within an event handler
and prevent the VM to keep running until they finish.

In order to do that, your event handler has to accept two arguments.
The first one will be the event object, and the second one a function.
The VM won't continue until you call this function.

If an exception is passed to that function, or thrown from within the
handler or a function called by it, the exception will bubble into the
VM and interrupt it, possibly corrupting its state. It's strongly
recommended not to do that.

### Synchronous event handlers

If you want to perform synchronous operations, you don't need
to receive a function as the handler's second argument, nor call it.

Note that if your event handler receives multiple arguments, the second
one will be the continuation function, and it must be called.

If an exception is thrown from withing the handler or a function called
by it, the exception will bubble into the VM and interrupt it, possibly
corrupting its state. It's strongly recommended not to throw from withing
event handlers.

# DEVELOPMENT

Developer documentation - currently mainly with information on testing and debugging - can be found [here](./developer.md).

# EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices.

If you want to join for work or do improvements on the libraries have a look at our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html).

# LICENSE

[MPL-2.0](https://www.mozilla.org/MPL/2.0/)
