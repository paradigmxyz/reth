---
reth-engine-local: patch
reth-node-builder: patch
---

Removed the `op` feature flag and `OpPayloadAttributes` `PayloadAttributesBuilder` implementation from `reth-engine-local`, along with the `op-alloy-rpc-types-engine` dependency. Updated `reth-node-builder` to no longer enable the removed `op` feature on `reth-engine-local`.
