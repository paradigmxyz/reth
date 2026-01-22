---
title: Make DatabaseStorageProof stateful
labels:
    - A-db
    - A-trie
    - C-enhancement
    - D-good-first-issue
assignees:
    - 18aaddy
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 12216
synced_at: 2026-01-21T11:32:15.98489Z
info:
    author: Rjected
    created_at: 2025-04-02T22:12:49Z
    updated_at: 2025-12-06T17:56:57Z
---

### Describe the feature

Right now `DatabaseStorageProof` has an associated `TX` type and takes `&'a Self::TX` on every method:
https://github.com/paradigmxyz/reth/blob/ca862ab9854080e6dfee98e8ef480fbd2a6d4887/crates/trie/db/src/proof.rs#L84-L104

This is not necessary, instead we can make this into a regular trait that is stateful and takes `&self`. For example it would look like:
```rust
/// Extends [`StorageProof`] with operations specific for working with a database transaction.
pub trait DatabaseStorageProof {
    /// Generates the storage proof for target slot based on [`TrieInput`].
    fn overlay_storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError>;

    /// Generates the storage multiproof for target slots based on [`TrieInput`].
    fn overlay_storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> Result<StorageMultiProof, StateProofError>;
}
```

And the implementers can have a `from_tx` method on the struct itself

### Additional context

_No response_
