---
reth-rpc-convert: patch
---

Updated `alloy-evm` dependency to git revision `9bc2dba` and adapted `TxEnvConverter` impl to the updated `TryIntoTxEnv` trait signature that now includes a `Spec` generic parameter.
