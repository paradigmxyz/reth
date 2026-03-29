---
reth-primitives-traits: major
reth-downloaders: patch
---

Removed the local `size` module from `reth-primitives-traits` and replaced it with `alloy_consensus::InMemorySize`. Simplified `SignedTransaction` to a blanket impl covering all types satisfying the required bounds, removing `is_system_tx`, `auto_impl` attributes, and explicit impls for `EthereumTxEnvelope` and OP types. Updated import paths in `reth-downloaders` accordingly.
