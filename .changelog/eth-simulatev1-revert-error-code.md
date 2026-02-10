---
reth-rpc-eth-types: minor
reth-rpc-e2e-tests: patch
---

Aligned `eth_simulateV1` revert error handling with the Execution APIs spec  
(see https://github.com/ethereum/execution-apis/pull/748).

EVM reverts now return JSON-RPC error code `3` instead of the generic `-32000`, while VM execution errors continue to return `-32015`.  
Updated the RPC compatibility test harness to propagate actual RPC error codes instead of masking them, enabling correct validation against execution-apis test cases.
