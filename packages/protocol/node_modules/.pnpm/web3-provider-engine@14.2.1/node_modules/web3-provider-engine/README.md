# Web3 ProviderEngine

[![Greenkeeper badge](https://badges.greenkeeper.io/MetaMask/provider-engine.svg)](https://greenkeeper.io/)

Web3 ProviderEngine is a tool for composing your own [web3 providers](https://github.com/ethereum/wiki/wiki/JavaScript-API#web3).


### Composable

Built to be modular - works via a stack of 'sub-providers' which are like normal web3 providers but only handle a subset of rpc methods.

The subproviders can emit new rpc requests in order to handle their own;  e.g. `eth_call` may trigger `eth_getAccountBalance`, `eth_getCode`, and others.
The provider engine also handles caching of rpc request results.

```js
const ProviderEngine = require('web3-provider-engine')
const CacheSubprovider = require('web3-provider-engine/subproviders/cache.js')
const FixtureSubprovider = require('web3-provider-engine/subproviders/fixture.js')
const FilterSubprovider = require('web3-provider-engine/subproviders/filters.js')
const VmSubprovider = require('web3-provider-engine/subproviders/vm.js')
const HookedWalletSubprovider = require('web3-provider-engine/subproviders/hooked-wallet.js')
const NonceSubprovider = require('web3-provider-engine/subproviders/nonce-tracker.js')
const RpcSubprovider = require('web3-provider-engine/subproviders/rpc.js')

var engine = new ProviderEngine()
var web3 = new Web3(engine)

// static results
engine.addProvider(new FixtureSubprovider({
  web3_clientVersion: 'ProviderEngine/v0.0.0/javascript',
  net_listening: true,
  eth_hashrate: '0x00',
  eth_mining: false,
  eth_syncing: true,
}))

// cache layer
engine.addProvider(new CacheSubprovider())

// filters
engine.addProvider(new FilterSubprovider())

// pending nonce
engine.addProvider(new NonceSubprovider())

// vm
engine.addProvider(new VmSubprovider())

// id mgmt
engine.addProvider(new HookedWalletSubprovider({
  getAccounts: function(cb){ ... },
  approveTransaction: function(cb){ ... },
  signTransaction: function(cb){ ... },
}))

// data source
engine.addProvider(new RpcSubprovider({
  rpcUrl: 'https://testrpc.metamask.io/',
}))

// log new blocks
engine.on('block', function(block){
  console.log('================================')
  console.log('BLOCK CHANGED:', '#'+block.number.toString('hex'), '0x'+block.hash.toString('hex'))
  console.log('================================')
})

// network connectivity error
engine.on('error', function(err){
  // report connectivity errors
  console.error(err.stack)
})

// start polling for blocks
engine.start()
```

When importing in webpack:
```js
import * as Web3ProviderEngine  from 'web3-provider-engine';
import * as RpcSource  from 'web3-provider-engine/subproviders/rpc';
import * as HookedWalletSubprovider from 'web3-provider-engine/subproviders/hooked-wallet';
```

### Built For Zero-Clients

The [Ethereum JSON RPC](https://github.com/ethereum/wiki/wiki/JSON-RPC) was not designed to have one node service many clients.
However a smaller, lighter subset of the JSON RPC can be used to provide the blockchain data that an Ethereum 'zero-client' node would need to function.
We handle as many types of requests locally as possible, and just let data lookups fallback to some data source ( hosted rpc, blockchain api, etc ).
Categorically, we don’t want / can’t have the following types of RPC calls go to the network:
* id mgmt + tx signing (requires private data)
* filters (requires a stateful data api)
* vm (expensive, hard to scale)

### Change Log

##### 14.0.0

- default dataProvider for zero is Infura mainnet REST api
- websocket support
- subscriptions support
- remove solc subprovider
- removed `dist` from git (but published in npm module)
- es5 builds in `dist/es5`
- zero + ProviderEngine bundles are es5
- web3 subprovider renamed to provider subprovider
- error if provider subprovider is missing a proper provider
- removed need to supply getAccounts hook
- fixed `hooked-wallet-ethtx` message signing
- fixed `hooked-wallet` default txParams

##### 13.0.0

- txs included in blocks via [`eth-block-tracker`](https://github.com/kumavis/eth-block-tracker)@2.0.0

##### 12.0.0

- moved block polling to [`eth-block-tracker`](https://github.com/kumavis/eth-block-tracker).

##### 11.0.0

- zero.js - replaced http subprovider with fetch provider (includes polyfill for node).

##### 10.0.0

- renamed HookedWalletSubprovider `personalRecoverSigner` to `recoverPersonalSignature`

##### 9.0.0

- `pollingShouldUnref` option now defaults to false
