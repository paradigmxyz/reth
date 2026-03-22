---
reth-rpc-eth-types: patch
---

Updated `eth_simulateV1` revert error code from `-32000` to `3` to be consistent with `eth_call`, per [execution-apis#748](https://github.com/ethereum/execution-apis/pull/748).
