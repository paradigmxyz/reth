# `txpool` Namespace

The `txpool` API allows you to inspect the transaction pool.

## `txpool_content`

Returns the details of all transactions currently pending for inclusion in the next block(s), as well as the ones that are being scheduled for future execution only.

See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool-content) for more details

| Client | Method invocation                            |
|--------|----------------------------------------------|
| RPC    | `{"method": "txpool_content", "params": []}` |

## `txpool_contentFrom`

Retrieves the transactions contained within the txpool, returning pending as well as queued transactions of this address, grouped by nonce.

See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool-contentfrom) for more details

| Client | Method invocation                                       |
|--------|---------------------------------------------------------|
| RPC    | `{"method": "txpool_contentFrom", "params": [address]}` |

## `txpool_inspect`

Returns a summary of all the transactions currently pending for inclusion in the next block(s), as well as the ones that are being scheduled for future execution only.

See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool-inspect) for more details

| Client | Method invocation                            |
|--------|----------------------------------------------|
| RPC    | `{"method": "txpool_inspect", "params": []}` |

## `txpool_status`

Returns the number of transactions currently pending for inclusion in the next block(s), as well as the ones that are being scheduled for future execution only.

See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool-status) for more details

| Client | Method invocation                           |
|--------|---------------------------------------------|
| RPC    | `{"method": "txpool_status", "params": []}` |