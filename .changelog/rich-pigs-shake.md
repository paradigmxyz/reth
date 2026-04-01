---
reth-payload-primitives: patch
reth-engine-local: patch
---

Removed the `op` feature and `op-alloy-rpc-types-engine` dependency from `reth-payload-primitives`, along with the `ExecutionPayload` impl for `OpExecutionData`. Updated `reth-engine-local` to drop the corresponding feature flag dependency.
