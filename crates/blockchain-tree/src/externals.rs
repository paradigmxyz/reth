//! Blockchain tree externals.

use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{consensus::Consensus, RethResult};
use reth_primitives::{BlockHash, BlockNumber};
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
    /// The provider factory, used to commit the canonical chain, or unwind it.
    pub(crate) provider_factory: ProviderFactory<DB>,
    /// The consensus engine.
    pub(crate) consensus: Arc<dyn Consensus>,
    /// The executor factory to execute blocks with.
    pub(crate) executor_factory: EF,
}

impl<DB, EF> TreeExternals<DB, EF> {
    /// Create new tree externals.
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        consensus: Arc<dyn Consensus>,
        executor_factory: EF,
    ) -> Self {
        Self { provider_factory, consensus, executor_factory }
    }
}

impl<DB: Database, EF> TreeExternals<DB, EF> {
    /// Fetches the latest canonical block hashes by walking backwards from the head.
    ///
    /// Returns the hashes sorted by increasing block numbers
    pub(crate) fn fetch_latest_canonical_hashes(
        &self,
        num_hashes: usize,
    ) -> RethResult<BTreeMap<BlockNumber, BlockHash>> {
        Ok(self
            .provider_factory
            .provider()?
            .tx_ref()
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take(num_hashes)
            .collect::<Result<BTreeMap<BlockNumber, BlockHash>, _>>()?)
    }
}
