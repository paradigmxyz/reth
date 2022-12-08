//! Provider that wraps around database traits.
//! to provide higher level abstraction over database tables.

mod block;
mod storage;
use std::sync::Arc;

pub use storage::{
    StateProviderImplHistory, StateProviderImplLatest, StateProviderImplRefHistory,
    StateProviderImplRefLatest,
};

use reth_db::database::Database;

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

#[cfg(test)]
mod tests {
    use crate::StateProviderFactory;

    use super::ProviderImpl;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};

    #[test]
    fn common_history_provider() {
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ProviderImpl::new(db);
        let _ = provider.latest();
    }
}
