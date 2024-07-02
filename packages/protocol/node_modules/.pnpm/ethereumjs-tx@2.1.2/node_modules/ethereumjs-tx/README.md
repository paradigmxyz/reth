# SYNOPSIS

[![NPM Package](https://img.shields.io/npm/v/ethereumjs-tx.svg?style=flat-square)](https://www.npmjs.org/package/ethereumjs-tx)
[![Actions Status](https://github.com/ethereumjs/ethereumjs-tx/workflows/tx-test/badge.svg)](https://github.com/ethereumjs/ethereumjs-tx/actions)
[![Coverage Status](https://img.shields.io/coveralls/ethereumjs/ethereumjs-tx.svg?style=flat-square)](https://coveralls.io/r/ethereumjs/ethereumjs-tx)
[![Gitter](https://img.shields.io/gitter/room/ethereum/ethereumjs-lib.svg?style=flat-square)](https://gitter.im/ethereum/ethereumjs-lib) or #ethereumjs on freenode

# INSTALL

`npm install ethereumjs-tx`

# USAGE

- [example](https://github.com/ethereumjs/ethereumjs-tx/blob/master/examples/transactions.ts)

```javascript
const EthereumTx = require('ethereumjs-tx').Transaction
const privateKey = Buffer.from(
  'e331b6d69882b4cb4ea581d88e0b604039a3de5967688d3dcffdd2270c0fd109',
  'hex',
)

const txParams = {
  nonce: '0x00',
  gasPrice: '0x09184e72a000',
  gasLimit: '0x2710',
  to: '0x0000000000000000000000000000000000000000',
  value: '0x00',
  data: '0x7f7465737432000000000000000000000000000000000000000000000000000000600057',
}

// The second parameter is not necessary if these values are used
const tx = new EthereumTx(txParams, { chain: 'mainnet', hardfork: 'petersburg' })
tx.sign(privateKey)
const serializedTx = tx.serialize()
```

# Chain and Hardfork Support

The `Transaction` and `FakeTransaction` constructors receives a second parameter that lets you specify the chain and hardfork
to be used. By default, `mainnet` and `petersburg` will be used.

There are two ways of customizing these. The first one, as shown in the previous section, is by
using an object with `chain` and `hardfork` names. You can see en example of this in [./examples/ropsten-tx.ts](./examples/ropsten-tx.ts).

The second option is by passing the option `common` set to an instance of [ethereumjs-common](https://github.com/ethereumjs/ethereumjs-common)' Common. This is specially useful for custom networks or chains/hardforks not yet supported by `ethereumjs-common`. You can see en example of this in [./examples/custom-chain-tx.ts](./examples/custom-chain-tx.ts).

## MuirGlacier Support

The `MuirGlacier` hardfork is supported by the library since the `v2.1.2` release.

## Istanbul Support

Support for reduced non-zero call data gas prices from the `Istanbul` hardfork
([EIP-2028](https://eips.ethereum.org/EIPS/eip-2028)) has been added to the library
along with the `v2.1.1` release.

# EIP-155 support

`EIP-155` replay protection is activated since the `spuriousDragon` hardfork. To disable it, set the
hardfork in the `Transaction`'s constructor.

# API

[./docs/](./docs/README.md)

# EthereumJS

See our organizational [documentation](https://ethereumjs.readthedocs.io) for an introduction to `EthereumJS` as well as information on current standards and best practices.

If you want to join for work or do improvements on the libraries have a look at our [contribution guidelines](https://ethereumjs.readthedocs.io/en/latest/contributing.html).

# LICENSE

[MPL-2.0](<https://tldrlegal.com/license/mozilla-public-license-2.0-(mpl-2)>)
