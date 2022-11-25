//! Provider that wraps around database traits.
//! to provide higher level abstraction over database tables.

mod block;
mod storage;
use std::sync::Arc;

pub use storage::{
    StateProviderImplHistory, StateProviderImplLatest, StateProviderImplRefHistory,
    StateProviderImplRefLatest,
};

use crate::db::Database;

/// Provider
pub struct ProviderImpl<DB: Database> {
    /// Database
    db: Arc<DB>,
}

impl<DB: Database> ProviderImpl<DB> {
    /// create new database provider
    pub fn new(db: Arc<DB>) -> Self {
        Self { db }
    }
}
