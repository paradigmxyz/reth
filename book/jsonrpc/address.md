# `address` Namespace

The `address` API provides methods to discover transactions relevant to particular addresses.

Once discovered, transactions can be obtained and examined using methods in other namespaces.

## Appearances
A relevant transaction is called an "appearance" and is defined
here: <https://github.com/ethereum/execution-apis/pull/456>

A set of appearances is a collection of transactions in which the address is involved in
an important way. An appearance includes:
- Sender or recipient of ether or tokens
- Contract caller or callee
- Contract deployer or deployed contract
- Block reward for block producer or miner/uncle
- Consensus layer withdrawal

An appearance is a transaction identifier consisting of a block number and transaction index.

## Node Size
> Note: The `address` namespace increases the node database size

The `address` namespace is separate because it involves the creation of additional database
tables for the index that maps addresses to appearances. Users may opt in to the namespace to
trigger the creation of the index and enable the associated JSON-RPC methods.

## `address_getAppearances`

Returns a set of transaction identifiers relevant to an address.

See also: <https://github.com/ethereum/execution-apis/pull/453>

| Client | Method invocation                                     |
|--------|-------------------------------------------------------|
| RPC    | `{"method": "address_getAppearances", "params": [address, opts]}` |



The block range can optionally be specified either by block number or tag (`latest`, `finalized`, `safe`, `earliest`, `pending`) as the second argument.

| Client | Method invocation                                     |
|--------|-------------------------------------------------------|
| RPC    | `{"method": "address_getAppearances", "params": [address, {"firstBlock": block, "lastBlock": block}}]}` |

### Examples

Discover appearances for address `0x30a4...1382`. Returns multiple transactions, the first is in block `14040913` (index `230`), the last is in block `17572135` (index `43`). Those
transactions can now be examined more closely using other JSON-RPC methods.
```js
// > {"jsonrpc":"2.0","id":1,"method":"address_getAppearances","params":["0x30a4639850b3ddeaaca4f06280aa751682f11382"]}
{
  "id": 1,
  "jsonrpc": "2.0",
  "result": [
    {
      "blockNumber": "0xd63f51",
      "transactionIndex": "0xe6"
    },
    ... // ommitted
    {
      "blockNumber": "0x10c2127",
      "transactionIndex": "0x2b"
    }
  ]
}
```

Discover appearances for address `0x30a4...1382`, specifically between blocks `17190873` and `17190889`. Two transactions are returned.
```js
// > {"jsonrpc":"2.0","id":1,"method":"address_getAppearances","params":["0x30a4639850b3ddeaaca4f06280aa751682f11382",{"firstBlock":"0x1064fd9","lastBlock":"0x1064fe9"}]}
{
  "id": 1,
  "jsonrpc": "2.0",
  "result": [
    {
      "blockNumber": "0x1064fd9",
      "transactionIndex": "0x5f"
    },
    {
      "blockNumber": "0x1064fe9",
      "transactionIndex": "0x59"
    }
  ]
}
```
