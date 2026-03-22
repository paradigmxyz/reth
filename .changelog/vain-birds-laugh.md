---
reth-engine-tree: patch
reth-evm-ethereum: patch
reth-evm: patch
reth-primitives-traits: patch
reth-revm: patch
reth-rpc-eth-api: patch
reth-rpc-eth-types: patch
reth-provider: patch
example-custom-evm: patch
example-precompile-cache: patch
---

Bumped revm to v35.0.0, revm-inspectors to 0.35.0, and alloy-evm to 0.29.0. Updated call sites throughout the codebase to align with the new APIs, including `ExecutionResult` field changes (`gas_used` → `gas.used()`/`gas.final_refunded()`), removal of `.without_state_clear()`, updated `EthPrecompiles::new(spec)` constructor, and updated `block_hashes.lowest()` access.
