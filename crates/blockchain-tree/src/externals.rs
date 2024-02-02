//! Blockchain tree externals.

use reth_db::{
    cursor::DbCursorRO, database::Database, snapshot::HeaderMask, tables, transaction::DbTx,
};
use reth_interfaces::{consensus::Consensus, RethResult};
use reth_primitives::{BlockHash, BlockNumber, SnapshotSegment};
use reth_provider::{ProviderFactory, StatsReader};
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
        let mut hashes = self
            .provider_factory
            .provider()?
            .tx_ref()
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take(num_hashes)
            .collect::<Result<BTreeMap<BlockNumber, BlockHash>, _>>()?;

        if hashes.len() < num_hashes {
            let snapshot_provider = self.provider_factory.snapshot_provider();
            let total_headers = snapshot_provider.count_entries::<tables::Headers>()? as u64;
            if total_headers > 0 {
                let range = total_headers
                    .saturating_sub(1)
                    .saturating_sub((num_hashes - hashes.len()) as u64)..
                    total_headers;

                hashes.extend(range.clone().zip(snapshot_provider.fetch_range_with_predicate(
                    SnapshotSegment::Headers,
                    range,
                    |cursor, number| cursor.get_one::<HeaderMask<BlockHash>>(number.into()),
                    |_| true,
                )?));
            }
        }

        Ok(hashes)
    }
}
