---
reth-consensus-debug-client: minor
reth-node-builder: minor
---

Refactored the debug consensus client to decouple forkchoice strategy from block providers. Introduced `RawBlockProvider` trait, `ForkchoiceProvider` wrapper, and `ForkchoiceMode` enum supporting `Offset`, `Finalized`, and `Tag` strategies. Added `FinalizedBlockBuffer` for buffering blocks until finalized, and updated `EtherscanBlockProvider` and `RpcBlockProvider` to implement `RawBlockProvider` with `BlockNumberOrTag`-based `get_block`.
