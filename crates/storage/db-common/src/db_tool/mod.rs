//! Common db operations

use boyer_moore_magiclen::BMByte;
use eyre::Result;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    table::{Decode, Decompress, DupSort, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseError, RawKey, RawTable, TableRawRow,
};
use reth_fs_util as fs;
use reth_node_types::NodeTypesWithDB;
use reth_provider::{providers::ProviderNodeTypes, ChainSpecProvider, DBProvider, ProviderFactory};
use std::{path::Path, rc::Rc, sync::Arc};
use tracing::info;

/// Wrapper over DB that implements many useful DB queries.
#[derive(Debug)]
pub struct DbTool<N: NodeTypesWithDB> {
    /// The provider factory that the db tool will use.
    pub provider_factory: ProviderFactory<N>,
}

impl<N: NodeTypesWithDB> DbTool<N> {
    /// Get an [`Arc`] to the underlying chainspec.
    pub fn chain(&self) -> Arc<N::ChainSpec> {
        self.provider_factory.chain_spec()
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [`HashMap`][std::collections::HashMap].
    ///
    /// [`ListFilter`] can be used to further
    /// filter down the desired results. (eg. List only rows which include `0xd3adbeef`)
    ///
    /// If `start_key` is provided, the query starts from that key (O(log n) seek) instead of
    /// scanning from the beginning. If `end_key` is provided, the query stops before that key.
    pub fn list<T: Table>(&self, filter: &ListFilter) -> Result<(Vec<TableRow<T>>, usize)> {
        let bmb = Rc::new(BMByte::from(&filter.search));
        if bmb.is_none() && filter.has_search() {
            eyre::bail!("Invalid search.")
        }

        let mut hits = 0;

        let data = self.provider_factory.db_ref().view(|tx| {
            let mut cursor =
                tx.cursor_read::<RawTable<T>>().expect("Was not able to obtain a cursor.");

            let end_key = filter.end_key.as_ref();
            let start_key = filter.start_key.as_ref();

            let map_filter = |row: Result<TableRawRow<T>, _>| {
                if let Ok((k, v)) = row {
                    let (key, value) = (k.into_key(), v.into_value());

                    // Check end_key boundary (exclusive)
                    if let Some(end) = end_key &&
                        &key >= end
                    {
                        return None
                    }

                    // Check start_key boundary (inclusive)
                    if let Some(start) = start_key &&
                        &key < start
                    {
                        return None
                    }

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
                // Reverse: seek to end_key and walk backwards toward start_key
                let seek_key: Option<RawKey<T::Key>> = filter.end_key.clone().map(RawKey::from_vec);
                Ok(cursor
                    .walk_back(seek_key)?
                    .skip(filter.skip)
                    .filter_map(map_filter)
                    .take(filter.len)
                    .collect::<Vec<(_, _)>>())
            } else {
                // Forward: seek to start_key and walk forward toward end_key
                let seek_key: Option<RawKey<T::Key>> =
                    filter.start_key.clone().map(RawKey::from_vec);
                Ok(cursor
                    .walk(seek_key)?
                    .skip(filter.skip)
                    .filter_map(map_filter)
                    .take(filter.len)
                    .collect::<Vec<(_, _)>>())
            }
        })?;

        Ok((data.map_err(|e: DatabaseError| eyre::eyre!(e))?, hits))
    }
}

impl<N: ProviderNodeTypes> DbTool<N> {
    /// Takes a DB where the tables have already been created.
    pub fn new(provider_factory: ProviderFactory<N>) -> eyre::Result<Self> {
        // Disable timeout because we are entering a TUI which might read for a long time. We
        // disable on the [`DbTool`] level since it's only used in the CLI.
        provider_factory.provider()?.disable_long_read_transaction_safety();
        Ok(Self { provider_factory })
    }

    /// Grabs the content of the table for the given key
    pub fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>> {
        self.provider_factory.db_ref().view(|tx| tx.get::<T>(key))?.map_err(|e| eyre::eyre!(e))
    }

    /// Grabs the content of the `DupSort` table for the given key and subkey
    pub fn get_dup<T: DupSort>(&self, key: T::Key, subkey: T::SubKey) -> Result<Option<T::Value>> {
        self.provider_factory
            .db_ref()
            .view(|tx| tx.cursor_dup_read::<T>()?.seek_by_key_subkey(key, subkey))?
            .map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database, the static files and ExEx WAL at the given paths.
    pub fn drop<P: AsRef<Path>>(
        &self,
        db_path: P,
        static_files_path: P,
        exex_wal_path: P,
    ) -> Result<()> {
        let db_path = db_path.as_ref();
        info!(target: "reth::cli", "Dropping database at {:?}", db_path);
        fs::remove_dir_all(db_path)?;

        let static_files_path = static_files_path.as_ref();
        info!(target: "reth::cli", "Dropping static files at {:?}", static_files_path);
        fs::remove_dir_all(static_files_path)?;
        fs::create_dir_all(static_files_path)?;

        if exex_wal_path.as_ref().exists() {
            let exex_wal_path = exex_wal_path.as_ref();
            info!(target: "reth::cli", "Dropping ExEx WAL at {:?}", exex_wal_path);
            fs::remove_dir_all(exex_wal_path)?;
        }

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
    /// Start key for range query (encoded as raw bytes). If provided, the query starts from this
    /// key instead of the beginning.
    pub start_key: Option<Vec<u8>>,
    /// End key for range query (encoded as raw bytes, exclusive). If provided, the query stops
    /// before this key.
    pub end_key: Option<Vec<u8>>,
}

impl ListFilter {
    /// If `search` has a list of bytes, then filter for rows that have this sequence.
    pub const fn has_search(&self) -> bool {
        !self.search.is_empty()
    }

    /// Updates the page with new `skip` and `len` values.
    pub const fn update_page(&mut self, skip: usize, len: usize) {
        self.skip = skip;
        self.len = len;
    }
}
