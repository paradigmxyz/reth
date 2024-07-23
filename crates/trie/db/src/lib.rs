//! An integration of [`reth-trie`] with [`reth-db`].

mod proof;
mod state;
mod storage;

pub use proof::DatabaseProof;
pub use state::DatabaseStateRoot;
pub use storage::DatabaseStorageRoot;
