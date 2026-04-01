---
reth-rpc-convert: minor
reth-storage-rpc-provider: minor
---

Replaced the separate `TryFromBlockResponse`, `TryFromReceiptResponse`, and `TryFromTransactionResponse` traits with a unified `RpcResponseConverter` trait and default `EthRpcConverter` implementation. Removed the `op-alloy-network` dependency and refactored `RpcBlockchainProvider` to store a dynamic converter instance instead of relying on per-type trait bounds.
