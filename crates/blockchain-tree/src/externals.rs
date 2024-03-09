//! Blockchain tree externals.

use reth_db::{
    cursor::DbCursorRO, database::Database, static_file::HeaderMask, tables, transaction::DbTx,
};
use reth_interfaces::{consensus::Consensus, RethResult};
use reth_primitives::{BlockHash, BlockNumber, StaticFileSegment};
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
pub struct TreeExternals<DB, EVM> {
    /// The provider factory, used to commit the canonical chain, or unwind it.
    pub(crate) provider_factory: ProviderFactory<DB>,
    /// The consensus engine.
    pub(crate) consensus: Arc<dyn Consensus>,
    /// The executor factory to execute blocks with.
    pub(crate) executor_factory: EVM,
}

impl<DB, EVM> TreeExternals<DB, EVM> {
    /// Create new tree externals.
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        consensus: Arc<dyn Consensus>,
        executor_factory: EVM,
    ) -> Self {
        Self { provider_factory, consensus, executor_factory }
    }
}

impl<DB: Database, EVM> TreeExternals<DB, EVM> {
    /// Fetches the latest canonical block hashes by walking backwards from the head.
    ///
    /// Returns the hashes sorted by increasing block numbers
    pub(crate) fn fetch_latest_canonical_hashes(
        &self,
        num_hashes: usize,
    ) -> RethResult<BTreeMap<BlockNumber, BlockHash>> {
        // Fetch the latest canonical hashes from the database
        let mut hashes = self
            .provider_factory
            .provider()?
            .tx_ref()
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take(num_hashes)
            .collect::<Result<BTreeMap<BlockNumber, BlockHash>, _>>()?;

        // Fetch the same number of latest canonical hashes from the static_files and merge them
        // with the database hashes. It is needed due to the fact that we're writing
        // directly to static_files in pipeline sync, but to the database in live sync,
        // which means that the latest canonical hashes in the static file might be more recent
        // than in the database, and vice versa, or even some ranges of the latest
        // `num_hashes` blocks may be in database, and some ranges in static_files.
        let static_file_provider = self.provider_factory.static_file_provider();
        let total_headers = static_file_provider.count_entries::<tables::Headers>()? as u64;
        if total_headers > 0 {
            let range =
                total_headers.saturating_sub(1).saturating_sub(num_hashes as u64)..total_headers;

            hashes.extend(range.clone().zip(static_file_provider.fetch_range_with_predicate(
                StaticFileSegment::Headers,
                range,
                |cursor, number| cursor.get_one::<HeaderMask<BlockHash>>(number.into()),
                |_| true,
            )?));
        }

        // We may have fetched more than `num_hashes` hashes, so we need to truncate the result to
        // the requested number.
        let hashes = hashes.into_iter().rev().take(num_hashes).collect();
        Ok(hashes)
    }
}
