//! Common db operations

use boyer_moore_magiclen::BMByte;
use eyre::Result;
use reth_chainspec::ChainSpec;
use reth_db::{RawTable, TableRawRow};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    table::{Decode, Decompress, DupSort, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_fs_util as fs;
use reth_provider::{ChainSpecProvider, ProviderFactory};
use std::{path::Path, rc::Rc, sync::Arc};
use tracing::info;

/// Wrapper over DB that implements many useful DB queries.
#[derive(Debug)]
pub struct DbTool<DB: Database> {
    /// The provider factory that the db tool will use.
    pub provider_factory: ProviderFactory<DB>,
}

impl<DB: Database> DbTool<DB> {
    /// Takes a DB where the tables have already been created.
    pub fn new(provider_factory: ProviderFactory<DB>) -> eyre::Result<Self> {
        // Disable timeout because we are entering a TUI which might read for a long time. We
        // disable on the [`DbTool`] level since it's only used in the CLI.
        provider_factory.provider()?.disable_long_read_transaction_safety();
        Ok(Self { provider_factory })
    }

    /// Get an [`Arc`] to the [`ChainSpec`].
    pub fn chain(&self) -> Arc<ChainSpec> {
        self.provider_factory.chain_spec()
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

    /// Grabs the content of the `DupSort` table for the given key and subkey
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
