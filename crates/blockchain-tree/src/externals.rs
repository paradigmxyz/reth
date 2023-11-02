//! Blockchain tree externals.

use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{consensus::Consensus, RethResult};
use reth_primitives::{BlockHash, BlockNumber, ChainSpec};
use reth_provider::ProviderFactory;
use std::{collections::BTreeMap, sync::Arc};

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
pub struct TreeExternals<DB, EF> {
    /// The database, used to commit the canonical chain, or unwind it.
    pub(crate) db: DB,
    /// The consensus engine.
    pub(crate) consensus: Arc<dyn Consensus>,
    /// The executor factory to execute blocks with.
    pub(crate) executor_factory: EF,
    /// The chain spec.
    pub(crate) chain_spec: Arc<ChainSpec>,
}

impl<DB, EF> TreeExternals<DB, EF> {
    /// Create new tree externals.
    pub fn new(
        db: DB,
        consensus: Arc<dyn Consensus>,
        executor_factory: EF,
        chain_spec: Arc<ChainSpec>,
    ) -> Self {
        Self { db, consensus, executor_factory, chain_spec }
    }
}

impl<DB: Database, EF> TreeExternals<DB, EF> {
    /// Return shareable database helper structure.
    pub fn database(&self) -> ProviderFactory<&DB> {
        ProviderFactory::new(&self.db, self.chain_spec.clone())
    }

    /// Fetches the latest canonical block hashes by walking backwards from the head.
    ///
    /// Returns the hashes sorted by increasing block numbers
    pub(crate) fn fetch_latest_canonical_hashes(
        &self,
        num_hashes: usize,
    ) -> RethResult<BTreeMap<BlockNumber, BlockHash>> {
        Ok(self
            .db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take(num_hashes)
            .collect::<Result<BTreeMap<BlockNumber, BlockHash>, _>>()?)
    }
}
