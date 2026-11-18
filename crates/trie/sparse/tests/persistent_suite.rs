//! Runs the unchanged generic suite against the persistent arena backend.

// Run the suite without changing its imports. Replace only the backend and thresholds;
// the rest of the library's public API is forwarded unchanged in this test crate.
extern crate reth_trie_sparse as sparse;
extern crate self as reth_trie_sparse;

pub use sparse::{
    PersistentArenaParallelSparseTrie as ArenaParallelSparseTrie,
    PersistentArenaParallelismThresholds as ArenaParallelismThresholds, *,
};

#[path = "suite/main.rs"]
mod suite;
