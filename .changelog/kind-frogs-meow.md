---
reth-primitives-traits: minor
reth-engine-local: patch
reth-evm: patch
reth-node-builder: patch
reth-payload-primitives: patch
reth-rpc-convert: patch
reth-rpc-eth-api: patch
reth-db-api: patch
reth-db: patch
---

Removed the unused `Extended` type and `op` feature (including `op-alloy-consensus` dependency) from `reth-primitives-traits`. Updated all dependent crates to remove the now-unnecessary `reth-primitives-traits/op` feature flag propagation.
