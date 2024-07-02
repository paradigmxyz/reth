[![npm Version](https://img.shields.io/npm/v/ganache-core.svg)](https://www.npmjs.com/package/ganache-core)
[![npm Downloads](https://img.shields.io/npm/dm/ganache-core.svg)](https://www.npmjs.com/package/ganache-core)
[![Build Status](https://travis-ci.org/trufflesuite/ganache-core.svg?branch=master)](https://travis-ci.org/trufflesuite/ganache-core)
# Ganache Core

This is the core code that powers the Ganache application and the Ganache command line tool.

[Usage](#usage) | [Options](#options) | [Implemented Methods](#implemented-methods) | [Custom Methods](#custom-methods) | [Unsupported Methods](#unsupported-methods) | [Testing](#testing)

## Installation

`ganache-core` is written in JavaScript and distributed as a Node.js package via `npm`. Make sure you have Node.js (>= v8.9.0) installed, and your environment is capable of installing and compiling `npm` modules.

**macOS** Make sure you have the XCode Command Line Tools installed. These are needed in general to be able to compile most C based languages on your machine, as well as many npm modules.

**Windows** See our [Windows install instructions](https://github.com/trufflesuite/ganache-cli/wiki/Installing-ganache-cli-on-Windows).

**Ubuntu/Linux** Follow the basic instructions for installing [Node.js](https://nodejs.org/en/download/package-manager/#debian-and-ubuntu-based-linux-distributions) and make sure that you have `npm` installed, as well as the `build-essential` `apt` package (it supplies `make` which you will need to compile most things). Use the official Node.js packages, *do not use the package supplied by your distribution.*

Using npm:

```Bash
npm install ganache-core
```

or, if you are using [Yarn](https://yarnpkg.com/):

```Bash
yarn add ganache-core
```


## Usage

As a [Web3](https://github.com/ethereum/web3.js/) provider:

```javascript
const ganache = require("ganache-core");
const web3 = new Web3(ganache.provider());
```
If web3 is already initialized:
```javascript
const ganache = require("ganache-core");
web3.setProvider(ganache.provider());
```
NOTE: depending on your web3 version, you may need to set a number of confirmation blocks
```javascript
const web3 = new Web3(provider, null, { transactionConfirmationBlocks: 1 });
```

As an [ethers.js](https://github.com/ethers-io/ethers.js/) provider:

```javascript
const ganache = require("ganache-core");
const provider = new ethers.providers.Web3Provider(ganache.provider());
```

As a general HTTP and WebSocket server:

```javascript
const ganache = require("ganache-core");
const server = ganache.server();
const provider = server.provider;
server.listen(port, function(err, blockchain) { ... });
```

## Options

Both `.provider()` and `.server()` take a single object which allows you to specify behavior of the Ganache instance. This parameter is optional. Available options are:

* `"accounts"`: `Array` of `Object`'s of the following shape: `{ secretKey: privateKey, balance: HexString }`.
  * If `secretKey` is specified, the key is used to determine the account's address. Otherwise, the address is auto-generated.
  * The `balance` is a hexadecimal value of the amount of Ether (in Wei) you want the account to be pre-loaded with.
* `"debug"`: `boolean` - Output VM opcodes for debugging
* `"blockTime"`: `number` - Specify blockTime in seconds for automatic mining. If you don't specify this flag, ganache will instantly mine a new block for every transaction. Using the `blockTime` option is discouraged unless you have tests which require a specific mining interval.
* `"logger"`: `Object` - Object, like `console`, that implements a `log()` function.
* `"mnemonic"`: Use a specific HD wallet mnemonic to generate initial addresses.
* `"port"`: `number` Port number to listen on when running as a server.
* `"seed"`: Use arbitrary data to generate the HD wallet mnemonic to be used.
* `"default_balance_ether"`: `number` - The default account balance, specified in ether.
* `"total_accounts"`: `number` - Number of accounts to generate at startup.
* `"fork"`: `string` or `object` - Fork from another currently running Ethereum client at a given block.  When a `string`, input should be the HTTP location and port of the other client, e.g. `http://localhost:8545`. You can optionally specify the block to fork from using an `@` sign: `http://localhost:8545@1599200`. Can also be a `Web3 Provider` object, optionally used in conjunction with the `fork_block_number` option below.
* `"fork_block_number"`: `string` or `number` - Block number the provider should fork from, when the `fork` option is specified. If the `fork` option is specified as a string including the `@` sign and a block number, the block number in the `fork` parameter takes precedence.
- `"forkCacheSize"`: `number` - The maximum size, in bytes, of the in-memory cache for queries on a chain fork. Defaults to `1_073_741_824` bytes (1 gigabyte). You can set this to `0` to disable caching (not recommended), or to `-1` for unlimited (will be limited by your node/browser process).
* `"network_id"`: Specify the network id ganache-core will use to identify itself (defaults to the current time or the network id of the forked blockchain if configured)
* `"_chainId"`: **(temporary option until v3)** Specify the chain's chainId. For legacy reasons, this does NOT affect the `eth_chainId` RPC response! Defaults to `1`
* `"_chainIdRpc"`: **(temporary option until v3)** Specify the `eth_chainId` RPC response value. For legacy reasons, this does NOT affect the chain's `chainid`! Defaults to `1337`
* `"time"`: `Date` - Date that the first block should start. Use this feature, along with the `evm_increaseTime` method to test time-dependent code.
* `"locked"`: `boolean` - whether or not accounts are locked by default.
* `"unlocked_accounts"`: `Array` - array of addresses or address indexes specifying which accounts should be unlocked.
* `"db_path"`: `String` - Specify a path to a directory to save the chain database. If a database already exists, `ganache-core` will initialize that chain instead of creating a new one. Note: You will not be able to modify state (accounts, balances, etc) on startup when you initialize ganache-core with a pre-existing database.
* `"db"`: `Object` - Specify an alternative database instance, for instance [MemDOWN](https://github.com/level/memdown).
* `"ws"`: `boolean` Enable a websocket server. This is `true` by default.
* `"account_keys_path"`: `String` - Specifies a file to save accounts and private keys to, for testing.
* `"vmErrorsOnRPCResponse"`: `boolean` - Whether or not to transmit transaction failures as RPC errors. Set to `false` for error reporting behaviour which is compatible with other clients such as geth and Parity. This is `true` by default to replicate the error reporting behavior of previous versions of ganache.
* `"hdPath"`: The hierarchical deterministic path to use when generating accounts. Default: "m/44'/60'/0'/0/"
* `"hardfork"`: `String` Allows users to specify which hardfork should be used. Supported hardforks are `byzantium`, `constantinople`, `petersburg`, `istanbul`, and `muirGlacier` (default).
* `"allowUnlimitedContractSize"`: `boolean` - Allows unlimited contract sizes while debugging (NOTE: this setting is often used in conjuction with an increased `gasLimit`). By setting this to `true`, the check within the EVM for contract size limit of 24KB (see [EIP-170](https://git.io/vxZkK)) is bypassed. Setting this to `true` **will** cause `ganache-core` to behave differently than production environments. (default: `false`; **ONLY** set to `true` during debugging).
* `"gasPrice"`: `String::hex` Sets the default gas price for transactions if not otherwise specified. Must be specified as a `hex` encoded string in `wei`. Defaults to `"0x77359400"` (2 `gwei`).
* `"gasLimit"`: `String::hex | number` Sets the block gas limit. Must be specified as a `hex` string or `number`(integer). Defaults to `"0x6691b7"`.
* `"callGasLimit"`: `number` Sets the transaction gas limit for `eth_call` and `eth_estimateGas` calls. Must be specified as a `hex` string. Defaults to `"0x1fffffffffffff"` (`Number.MAX_SAFE_INTEGER`).
* `"keepAliveTimeout"`:  `number` If using `.server()` - Sets the HTTP server's `keepAliveTimeout` in milliseconds. See the [NodeJS HTTP docs](https://nodejs.org/api/http.html#http_server_keepalivetimeout) for details. `5000` by default.

## Implemented Methods

The RPC methods currently implemented are:

* [eth_accounts](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_accounts)
* [eth_blockNumber](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_blockNumber)
* [eth_call](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_call)
* `eth_chainId`
* [eth_coinbase](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_coinbase)
* [eth_estimateGas](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_estimateGas)
* [eth_gasPrice](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_gasPrice)
* [eth_getBalance](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getBalance)
* [eth_getBlockByNumber](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getBlockByNumber)
* [eth_getBlockByHash](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getBlockByHash)
* [eth_getBlockTransactionCountByHash](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getBlockTransactionCountByHash)
* [eth_getBlockTransactionCountByNumber](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getBlockTransactionCountByNumber)
* [eth_getCode](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getCode)
* [eth_getCompilers](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getCompilers)
* [eth_getFilterChanges](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getFilterChanges)
* [eth_getFilterLogs](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getFilterLogs)
* [eth_getLogs](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getLogs)
* [eth_getStorageAt](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getStorageAt)
* [eth_getTransactionByHash](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getTransactionByHash)
* [eth_getTransactionByBlockHashAndIndex](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getTransactionByBlockHashAndIndex)
* [eth_getTransactionByBlockNumberAndIndex](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getTransactionByBlockNumberAndIndex)
* [eth_getTransactionCount](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getTransactionCount)
* [eth_getTransactionReceipt](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getTransactionReceipt)
* [eth_hashrate](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_hashrate)
* [eth_mining](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_mining)
* [eth_newBlockFilter](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_newBlockFilter)
* [eth_newFilter](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_newFilter)
* [eth_protocolVersion](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_protocolVersion)
* [eth_sendTransaction](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_sendTransaction)
* [eth_sendRawTransaction](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_sendRawTransaction)
* [eth_sign](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_sign)
* `eth_signTypedData`
* [eth_subscribe](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_subscribe)
* [eth_unsubscribe](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_unsubscribe)
* [shh_version](https://github.com/ethereum/wiki/wiki/JSON-RPC#shh_version)
* [net_version](https://github.com/ethereum/wiki/wiki/JSON-RPC#net_version)
* [net_peerCount](https://github.com/ethereum/wiki/wiki/JSON-RPC#net_peerCount)
* [net_listening](https://github.com/ethereum/wiki/wiki/JSON-RPC#net_listening)
* [eth_uninstallFilter](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_uninstallFilter)
* [eth_syncing](https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_syncing)
* [web3_clientVersion](https://github.com/ethereum/wiki/wiki/JSON-RPC#web3_clientVersion)
* [web3_sha3](https://github.com/ethereum/wiki/wiki/JSON-RPC#web3_sha3)
* `bzz_hive`
* `bzz_info`

#### Management API Methods

* [debug_traceTransaction](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#debug_tracetransaction)
* [miner_start](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#miner_start)
* [miner_stop](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#miner_stop)
* [personal_sendTransaction](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#personal_sendTransaction)
* [personal_unlockAccount](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#personal_unlockAccount)
* [personal_importRawKey](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#personal_importRawKey)
* [personal_newAccount](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#personal_newAccount)
* [personal_lockAccount](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#personal_lockAccount)
* [personal_listAccounts](https://github.com/ethereum/go-ethereum/wiki/Management-APIs#personal_listaccounts)

## Custom Methods

Special non-standard methods that aren’t included within the original RPC specification:
* `evm_snapshot` : Snapshot the state of the blockchain at the current block. Takes no parameters. Returns the integer id of the snapshot created. A snapshot can only be used once. After a successful `evm_revert`, the same snapshot id cannot be used again. Consider creating a new snapshot after each `evm_revert` *if you need to revert to the same point multiple times*.
  ```bash
  curl -H "Content-Type: application/json" -X POST --data \
          '{"id":1337,"jsonrpc":"2.0","method":"evm_snapshot","params":[]}' \
          http://localhost:8545
  ```
  ```json
  { "id": 1337, "jsonrpc": "2.0", "result": "0x1" }
  ```
* `evm_revert` : Revert the state of the blockchain to a previous snapshot. Takes a single parameter, which is the snapshot id to revert to. This deletes the given snapshot, as well as any snapshots taken after (Ex: reverting to id `0x1` will delete snapshots with ids `0x1`, `0x2`, `etc`...  If no snapshot id is passed it will revert to the latest snapshot. Returns `true`.
  ```bash
  # Ex: ID "1" (hex encoded string)
  curl -H "Content-Type: application/json" -X POST --data \
          '{"id":1337,"jsonrpc":"2.0","method":"evm_revert","params":["0x1"]}' \
          http://localhost:8545
  ```
  ```json
  { "id": 1337, "jsonrpc": "2.0", "result": true }
  ```
* `evm_increaseTime` : Jump forward in time. Takes one parameter, which is the amount of time to increase in seconds. Returns the total time adjustment, in seconds.
  ```bash
  # Ex: 1 minute (number)
  curl -H "Content-Type: application/json" -X POST --data \
          '{"id":1337,"jsonrpc":"2.0","method":"evm_increaseTime","params":[60]}' \
          http://localhost:8545
  ```
  ```json
  { "id": 1337, "jsonrpc": "2.0", "result": "060" }
  ```
* `evm_mine` : Force a block to be mined (independent of mining status: started | stopped). Takes one **optional** parameter, which is the timestamp a block should setup as the mining time. NOTE: the timestamp parameter should be specified in `seconds`. In JavaScript you would calculate it like this: `Math.floor(Date.now() / 1000);`
  ```bash
  # Ex: new Date("2009-01-03T18:15:05+00:00").getTime()
  curl -H "Content-Type: application/json" -X POST --data \
          '{"id":1337,"jsonrpc":"2.0","method":"evm_mine","params":[1231006505000]}' \
          http://localhost:8545
  ```

  ```json
  { "id": 1337, "jsonrpc": "2.0", "result": "0x0" }
  ```
* `evm_unlockUnknownAccount` : Unlocks any unknown account. Accounts known to the `personal` namespace and accounts
returned by `eth_accounts` cannot be unlocked using this method; use `personal_lockAccount` instead.
  ```bash
  # Ex: account: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e"
  curl -H "Content-Type: application/json" -X POST --data \
          '{"id":1337,"jsonrpc":"2.0","method":"evm_unlockUnknownAccount","params":["0x742d35Cc6634C0532925a3b844Bc454e4438f44e"]}' \
          http://localhost:8545
  ```

  ```json
  { "id": 1337, "jsonrpc": "2.0", "result": true }
  ```
* `evm_lockUnknownAccount` : Locks any unknown account. Accounts known to the `personal` namespace and accounts
returned by `eth_accounts` cannot be locked using this method; use `personal_unlockAccount` instead.
  ```bash
  # Ex: account: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e"
  curl -H "Content-Type: application/json" -X POST --data \
          '{"id":1337,"jsonrpc":"2.0","method":"evm_lockUnknownAccount","params":["0x742d35Cc6634C0532925a3b844Bc454e4438f44e"]}' \
          http://localhost:8545
  ```

  ```json
  { "id": 1337, "jsonrpc": "2.0", "result": true }
  ```

## Unsupported Methods

* `eth_compileSolidity`: If you'd like Solidity compilation in Javascript, please see the [solc-js project](https://github.com/ethereum/solc-js).


## Testing

Run tests via:

```
$ npm test
```

## License
[MIT](https://tldrlegal.com/license/mit-license)
