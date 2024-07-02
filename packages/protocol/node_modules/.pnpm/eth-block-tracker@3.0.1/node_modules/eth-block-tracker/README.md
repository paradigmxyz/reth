
# eth-block-tracker

This module walks the Ethereum blockchain, keeping track of the latest block.
It uses a web3 provider as a data source and will continuously poll for the next block.

```js
const HttpProvider = require('ethjs-provider-http')
const BlockTracker = require('eth-block-tracker')

const provider = new HttpProvider('https://mainnet.infura.io')
const blockTracker = new BlockTracker({ provider })
blockTracker.on('block', console.log)
blockTracker.start()
```

### methods

##### new BlockTracker({ provider, pollingInterval })

creates a new block tracker with `provider` as a data source and
`pollingInterval` (ms) timeout between polling for the latest block.

##### getCurrentBlock()

synchronous returns the current block. may be `null`.

```js
console.log(blockTracker.getCurrentBlock())
```

##### awaitCurrentBlock()

Returns a promise. asynchronously returns the current block.
if not yet available, it will wait until it has the latest block.

##### start({ fromBlock })

Start walking from the `fromBlock` (default: `'latest'`) forward.
`fromBlock` should be a number as a hex encoded string.
Returns a promise which is rejected if an error in encountered.

```js
blockTracker.start()
```

```js
blockTracker.start({ fromBlock: '0x00' })
```

##### stop()

Stop walking the blockchain.

```js
blockTracker.stop()
```

### EVENTS

##### block

The `block` event is emitted for every block in order.
Use this event if you want to operate on every block without missing any.

```js
blockTracker.on('block', (newBlock) => console.log(newBlock))
```

##### latest

The `latest` event is emitted for every that is detected to be the latest block.
This means skipping a block if there were two created since the last polling period.
Use this event if you don't care about stale blocks.

```js
blockTracker.on('latest', (newBlock) => console.log(newBlock))
```

##### sync

The `sync` event is emitted the same as "latest" but includes the previous block.

```js
blockTracker.on('sync', ({ newBlock, oldBlock }) => console.log(newBlock, oldBlock))
```

### NOTES

Does not currently handle block reorgs.
