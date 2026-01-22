---
title: Make DatabaseTrieWitness stateful
labels:
    - A-db
    - A-trie
    - C-enhancement
    - D-good-first-issue
assignees:
    - cy00r
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 12216
synced_at: 2026-01-21T11:32:15.985115Z
info:
    author: Rjected
    created_at: 2025-04-02T22:18:23Z
    updated_at: 2025-12-08T11:40:56Z
---

### Describe the feature

Right now `DatabaseTrieWitness` has an associated `TX` type and takes `&'a Self::TX` on every method:
https://github.com/paradigmxyz/reth/blob/ca862ab9854080e6dfee98e8ef480fbd2a6d4887/crates/trie/db/src/witness.rs#L10-L21

This is not necessary, instead we can make this into a regular trait that is stateful and takes `&self`. For example it would look like:
```rust
/// Extends [`TrieWitness`] with operations specific for working with a database transaction.
pub trait DatabaseTrieWitness {
    /// Generates trie witness for target state based on [`TrieInput`].
    fn overlay_witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}
```

And the implementers can have a `from_tx` method on the struct itself

### Additional context

_No response_
