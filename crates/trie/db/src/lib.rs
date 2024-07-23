//! An integration of [`reth-trie`] with [`reth-db`].

mod state;
mod storage;

pub use state::DatabaseStateRoot;
pub use storage::DatabaseStorageRoot;
