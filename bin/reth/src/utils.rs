//! Common CLI utility functions.

use boyer_moore_magiclen::BMByte;
use eyre::Result;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_config::Config;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    table::{Decode, Decompress, DupSort, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseEnv, DatabaseError, RawTable, TableRawRow,
};
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_evm::noop::NoopBlockExecutorProvider;
use reth_fs_util as fs;
use reth_primitives::{stage::PipelineTarget, ChainSpec};
use reth_provider::{
    providers::StaticFileProvider, HeaderSyncMode, ProviderFactory, StaticFileProviderFactory,
};
use reth_stages::{sets::DefaultStages, Pipeline};
use reth_static_file::StaticFileProducer;
use std::{path::Path, rc::Rc, sync::Arc};
use tracing::info;

/// Exposing `open_db_read_only` function
pub mod db {
    pub use reth_db::open_db_read_only;
}

/// Re-exported from `reth_node_core`, also to prevent a breaking change. See the comment on
/// the `reth_node_core::args` re-export for more details.
pub use reth_node_core::utils::*;

/// Wrapper over DB that implements many useful DB queries.
#[derive(Debug)]
pub struct DbTool<DB: Database> {
    /// The provider factory that the db tool will use.
    pub provider_factory: ProviderFactory<DB>,
    /// The [ChainSpec] that the db tool will use.
    pub chain: Arc<ChainSpec>,
}

impl<DB: Database> DbTool<DB> {
    /// Takes a DB where the tables have already been created.
    pub fn new(provider_factory: ProviderFactory<DB>, chain: Arc<ChainSpec>) -> eyre::Result<Self> {
        // Disable timeout because we are entering a TUI which might read for a long time. We
        // disable on the [`DbTool`] level since it's only used in the CLI.
        provider_factory.provider()?.disable_long_read_transaction_safety();
        Ok(Self { provider_factory, chain })
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [`HashMap`][std::collections::HashMap].
    ///
    /// [`ListFilter`] can be used to further
    /// filter down the desired results. (eg. List only rows which include `0xd3adbeef`)
    pub fn list<T: Table>(&self, filter: &ListFilter) -> Result<(Vec<TableRow<T>>, usize)> {
        let bmb = Rc::new(BMByte::from(&filter.search));
        if bmb.is_none() && filter.has_search() {
            eyre::bail!("Invalid search.")
        }

        let mut hits = 0;

        let data = self.provider_factory.db_ref().view(|tx| {
            let mut cursor =
                tx.cursor_read::<RawTable<T>>().expect("Was not able to obtain a cursor.");

            let map_filter = |row: Result<TableRawRow<T>, _>| {
                if let Ok((k, v)) = row {
                    let (key, value) = (k.into_key(), v.into_value());

                    if key.len() + value.len() < filter.min_row_size {
                        return None
                    }
                    if key.len() < filter.min_key_size {
                        return None
                    }
                    if value.len() < filter.min_value_size {
                        return None
                    }

                    let result = || {
                        if filter.only_count {
                            return None
                        }
                        Some((
                            <T as Table>::Key::decode(&key).unwrap(),
                            <T as Table>::Value::decompress(&value).unwrap(),
                        ))
                    };

                    match &*bmb {
                        Some(searcher) => {
                            if searcher.find_first_in(&value).is_some() ||
                                searcher.find_first_in(&key).is_some()
                            {
                                hits += 1;
                                return result()
                            }
                        }
                        None => {
                            hits += 1;
                            return result()
                        }
                    }
                }
                None
            };

            if filter.reverse {
                Ok(cursor
                    .walk_back(None)?
                    .skip(filter.skip)
                    .filter_map(map_filter)
                    .take(filter.len)
                    .collect::<Vec<(_, _)>>())
            } else {
                Ok(cursor
                    .walk(None)?
                    .skip(filter.skip)
                    .filter_map(map_filter)
                    .take(filter.len)
                    .collect::<Vec<(_, _)>>())
            }
        })?;

        Ok((data.map_err(|e: DatabaseError| eyre::eyre!(e))?, hits))
    }

    /// Grabs the content of the table for the given key
    pub fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>> {
        self.provider_factory.db_ref().view(|tx| tx.get::<T>(key))?.map_err(|e| eyre::eyre!(e))
    }

    /// Grabs the content of the DupSort table for the given key and subkey
    pub fn get_dup<T: DupSort>(&self, key: T::Key, subkey: T::SubKey) -> Result<Option<T::Value>> {
        self.provider_factory
            .db_ref()
            .view(|tx| tx.cursor_dup_read::<T>()?.seek_by_key_subkey(key, subkey))?
            .map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database and the static files at the given path.
    pub fn drop(
        &self,
        db_path: impl AsRef<Path>,
        static_files_path: impl AsRef<Path>,
    ) -> Result<()> {
        let db_path = db_path.as_ref();
        info!(target: "reth::cli", "Dropping database at {:?}", db_path);
        fs::remove_dir_all(db_path)?;

        let static_files_path = static_files_path.as_ref();
        info!(target: "reth::cli", "Dropping static files at {:?}", static_files_path);
        fs::remove_dir_all(static_files_path)?;
        fs::create_dir_all(static_files_path)?;

        Ok(())
    }

    /// Drops the provided table from the database.
    pub fn drop_table<T: Table>(&self) -> Result<()> {
        self.provider_factory.db_ref().update(|tx| tx.clear::<T>())??;
        Ok(())
    }
}

/// Filters the results coming from the database.
#[derive(Debug)]
pub struct ListFilter {
    /// Skip first N entries.
    pub skip: usize,
    /// Take N entries.
    pub len: usize,
    /// Sequence of bytes that will be searched on values and keys from the database.
    pub search: Vec<u8>,
    /// Minimum row size.
    pub min_row_size: usize,
    /// Minimum key size.
    pub min_key_size: usize,
    /// Minimum value size.
    pub min_value_size: usize,
    /// Reverse order of entries.
    pub reverse: bool,
    /// Only counts the number of filtered entries without decoding and returning them.
    pub only_count: bool,
}

impl ListFilter {
    /// If `search` has a list of bytes, then filter for rows that have this sequence.
    pub fn has_search(&self) -> bool {
        !self.search.is_empty()
    }

    /// Updates the page with new `skip` and `len` values.
    pub fn update_page(&mut self, skip: usize, len: usize) {
        self.skip = skip;
        self.len = len;
    }
}

/// Returns a [ProviderFactory] after executing consistency checks.
///
/// If it's a read-write environment and an issue is found, it will attempt to heal (including a
/// pipeline unwind). Otherwise it will thrown an error.
pub fn create_provider_factory(
    config: &Config,
    chain_spec: Arc<ChainSpec>,
    db: Arc<DatabaseEnv>,
    static_file_provider: StaticFileProvider,
) -> eyre::Result<ProviderFactory<Arc<DatabaseEnv>>> {
    if db.is_read_only() != static_file_provider.is_read_only() {
        return Err(eyre::eyre!("Storage types should be open with same access rights."));
    }

    let has_receipt_pruning = config.prune.as_ref().map_or(false, |a| a.has_receipts_pruning());
    let factory = ProviderFactory::new(db, chain_spec.clone(), static_file_provider);

    info!(target: "reth::cli", "Verifying storage consistency.");

    // Check for consistency between database and static files.
    if let Some(unwind_target) = factory
        .static_file_provider()
        .check_consistency(&factory.provider()?, has_receipt_pruning)?
    {
        if factory.db_ref().is_read_only() || factory.static_file_provider().is_read_only() {
            return Err(eyre::eyre!("Inconsistent storage. Restart node to heal: {unwind_target}"));
        }

        let prune_modes = config.prune.clone().map(|prune| prune.segments).unwrap_or_default();

        // Highly unlikely to happen, and given its destructive nature, it's better to panic
        // instead.
        if PipelineTarget::Unwind(0) == unwind_target {
            panic!("A static file <> database inconsistency was found that would trigger an unwind to block 0.")
        }

        info!(target: "reth::cli", unwind_target = %unwind_target, "Executing an unwind after a failed storage consistency check.");

        // Builds and executes an unwind-only pipeline
        let mut pipeline = Pipeline::builder()
            .add_stages(DefaultStages::new(
                factory.clone(),
                HeaderSyncMode::Continuous,
                Arc::new(EthBeaconConsensus::new(chain_spec)),
                NoopHeaderDownloader::default(),
                NoopBodiesDownloader::default(),
                NoopBlockExecutorProvider::default(),
                config.stages.clone(),
                prune_modes.clone(),
            ))
            .build(
                factory.clone(),
                StaticFileProducer::new(
                    factory.clone(),
                    factory.static_file_provider(),
                    prune_modes,
                ),
            );

        // Move all applicable data from database to static files.
        pipeline.move_to_static_files()?;
        pipeline.unwind(unwind_target.unwind_target().expect("should exist"), None)?;
    }

    Ok(factory)
}
