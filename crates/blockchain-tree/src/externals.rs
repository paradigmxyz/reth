//! Blockchain tree externals.

use reth_db::{database::Database, tables, transaction::DbTx};
use reth_primitives::{ChainSpec, SealedHeader};
use reth_provider::{HeaderProvider, ShareableDatabase};
use std::sync::Arc;

/// A container for external components.
///
/// This is a simple container for external components used throughout the blockchain tree
/// implementation:
///
/// - A handle to the database
/// - A handle to the consensus engine
/// - The executor factory to execute blocks with
/// - The chain spec
#[derive(Debug)]
pub struct TreeExternals<DB, C, EF> {
    /// The database, used to commit the canonical chain, or unwind it.
    pub(crate) db: DB,
    /// The consensus engine.
    pub(crate) consensus: C,
    /// The executor factory to execute blocks with.
    pub(crate) executor_factory: EF,
    /// The chain spec.
    pub(crate) chain_spec: Arc<ChainSpec>,
}

impl<DB, C, EF> TreeExternals<DB, C, EF> {
    /// Create new tree externals.
    pub fn new(db: DB, consensus: C, executor_factory: EF, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, consensus, executor_factory, chain_spec }
    }
}

impl<DB: Database, C, EF> TreeExternals<DB, C, EF> {
    /// Reads the last `max` canonical headers from the database.
    ///
    /// Note: This returns the headers in rising order, i.e. the highest header is the last entry in
    /// the vec.
    pub(crate) fn read_last_canonical_headers(
        &self,
        max: u64,
    ) -> Result<Vec<SealedHeader>, reth_interfaces::Error> {
        let (highest, _) = self.db.tx()?.cursor_read::<tables::CanonicalHeaders>()?.last()?;
        let range = highest.saturating_sub(max)..=highest;

        self.database().sealed_headers_range(range)
    }

    /// Return shareable database helper structure.
    pub fn database(&self) -> ShareableDatabase<&DB> {
        ShareableDatabase::new(&self.db, self.chain_spec.clone())
    }
}
