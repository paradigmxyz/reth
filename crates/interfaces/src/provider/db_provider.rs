//! Provider that wraps around database traits.
//! to provide higher level abstraction over database tables.

mod block;
mod storage;
pub use storage::{StateProviderImplHistory, StateProviderImplLatest};

use crate::db::Database;

/// Provider
pub struct ProviderImpl<DB: Database> {
    /// Database
    db: DB,
}

impl<DB: Database> ProviderImpl<DB> {
    /// create new database provider
    pub fn new(db: DB) -> Self {
        Self { db }
    }
}
