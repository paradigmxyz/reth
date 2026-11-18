//! Runs the unchanged generic suite against the persistent arena backend.

// The suite imports its backend by name. Redirect that import in this test crate only.
extern crate reth_trie_sparse as sparse;
extern crate self as reth_trie_sparse;

pub use sparse::{
    LeafLookup, LeafLookupError, LeafUpdate,
    PersistentArenaParallelSparseTrie as ArenaParallelSparseTrie,
    PersistentArenaParallelismThresholds as ArenaParallelismThresholds, SparseTrie, TrieNodeEpoch,
};

#[path = "suite/main.rs"]
mod suite;
