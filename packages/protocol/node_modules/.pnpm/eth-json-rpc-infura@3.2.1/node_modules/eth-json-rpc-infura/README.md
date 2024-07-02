# eth-json-rpc-infura

`json-rpc-engine` middleware for infura's REST endpoints.

### usage as provider

```js
const createInfuraProvider = require('eth-json-rpc-infura/src/createProvider')
const Ethjs = require('ethjs')

const provider = createInfuraProvider({ network: 'mainnet' })
const eth = new Ethjs(provider)
```

### usage as middleware

```js
const createInfuraMiddleware = require('eth-json-rpc-infura')
const RpcEngine = require('json-rpc-engine')

const engine = new RpcEngine()
engine.push(createInfuraMiddleware({ network: 'ropsten' }))
```
