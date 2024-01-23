//! Common CLI utility functions.

use boyer_moore_magiclen::BMByte;
use eyre::Result;
use reth_consensus_common::validation::validate_block_standalone;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    table::{Decode, Decompress, DupSort, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseError, RawTable, TableRawRow,
};
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_network::NetworkManager;
use reth_primitives::{
    fs, BlockHashOrNumber, ChainSpec, HeadersDirection, SealedBlock, SealedHeader,
};
use reth_provider::BlockReader;
use reth_rpc::{JwtError, JwtSecret};
use std::{
    env::VarError,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use tracing::{debug, info, trace, warn};

/// Exposing `open_db_read_only` function
pub mod db {
    pub use reth_db::open_db_read_only;
}

/// Get a single header from network
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> Result<SealedHeader>
where
    Client: HeadersClient,
{
    let request = HeadersRequest { direction: HeadersDirection::Rising, limit: 1, start: id };

    let (peer_id, response) =
        client.get_headers_with_priority(request, Priority::High).await?.split();

    if response.len() != 1 {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: {}", response.len())
    }

    let header = response.into_iter().next().unwrap().seal_slow();

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number == number,
    };

    if !valid {
        client.report_bad_message(peer_id);
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}

/// Get a body from network based on header
pub async fn get_single_body<Client>(
    client: Client,
    chain_spec: Arc<ChainSpec>,
    header: SealedHeader,
) -> Result<SealedBlock>
where
    Client: BodiesClient,
{
    let (peer_id, response) = client.get_block_body(header.hash).await?.split();

    if response.is_none() {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of bodies received. Expected: 1. Received: 0")
    }

    let block = response.unwrap();
    let block = SealedBlock {
        header,
        body: block.transactions,
        ommers: block.ommers,
        withdrawals: block.withdrawals,
    };

    validate_block_standalone(&block, &chain_spec)?;

    Ok(block)
}

/// Wrapper over DB that implements many useful DB queries.
#[derive(Debug)]
pub struct DbTool<'a, DB: Database> {
    /// The database that the db tool will use.
    pub db: &'a DB,
    /// The [ChainSpec] that the db tool will use.
    pub chain: Arc<ChainSpec>,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created.
    pub fn new(db: &'a DB, chain: Arc<ChainSpec>) -> eyre::Result<Self> {
        Ok(Self { db, chain })
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

        let data = self.db.view(|tx| {
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
        self.db.view(|tx| tx.get::<T>(key))?.map_err(|e| eyre::eyre!(e))
    }

    /// Grabs the content of the DupSort table for the given key and subkey
    pub fn get_dup<T: DupSort>(&self, key: T::Key, subkey: T::SubKey) -> Result<Option<T::Value>> {
        self.db
            .view(|tx| tx.cursor_dup_read::<T>()?.seek_by_key_subkey(key, subkey))?
            .map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database at the given path.
    pub fn drop(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        info!(target: "reth::cli", "Dropping database at {:?}", path);
        fs::remove_dir_all(path)?;
        Ok(())
    }

    /// Drops the provided table from the database.
    pub fn drop_table<T: Table>(&mut self) -> Result<()> {
        self.db.update(|tx| tx.clear::<T>())??;
        Ok(())
    }
}

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, shellexpand::LookupError<VarError>> {
    shellexpand::full(value).map(|path| PathBuf::from(path.into_owned()))
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

/// Attempts to retrieve or create a JWT secret from the specified path.
pub fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
    if path.exists() {
        debug!(target: "reth::cli", ?path, "Reading JWT auth secret file");
        JwtSecret::from_file(path)
    } else {
        info!(target: "reth::cli", ?path, "Creating JWT auth secret file");
        JwtSecret::try_create(path)
    }
}

/// Collect the peers from the [NetworkManager] and write them to the given `persistent_peers_file`,
/// if configured.
pub fn write_peers_to_file<C>(network: &NetworkManager<C>, persistent_peers_file: Option<PathBuf>)
where
    C: BlockReader + Unpin,
{
    if let Some(file_path) = persistent_peers_file {
        let known_peers = network.all_peers().collect::<Vec<_>>();
        if let Ok(known_peers) = serde_json::to_string_pretty(&known_peers) {
            trace!(target: "reth::cli", peers_file =?file_path, num_peers=%known_peers.len(), "Saving current peers");
            let parent_dir = file_path.parent().map(fs::create_dir_all).transpose();
            match parent_dir.and_then(|_| fs::write(&file_path, known_peers)) {
                Ok(_) => {
                    info!(target: "reth::cli", peers_file=?file_path, "Wrote network peers to file");
                }
                Err(err) => {
                    warn!(target: "reth::cli", ?err, peers_file=?file_path, "Failed to write network peers to file");
                }
            }
        }
    }
}
