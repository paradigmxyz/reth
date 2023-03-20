//! Common CLI utility functions.
use crate::dirs::{DbPath, PlatformPath};
use eyre::{Result, WrapErr};
use reth_db::{
    cursor::{DbCursorRO, Walker},
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    p2p::{
        download::DownloadClient,
        headers::client::{HeadersClient, HeadersRequest},
        priority::Priority,
    },
    test_utils::generators::random_block_range,
};
use reth_network::FetchClient;
use reth_primitives::{BlockHashOrNumber, HeadersDirection, SealedHeader};
use reth_provider::insert_canonical_block;
use std::collections::BTreeMap;
use tracing::info;

/// Get a single header from network
pub async fn get_single_header(
    client: FetchClient,
    id: BlockHashOrNumber,
) -> eyre::Result<SealedHeader> {
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

/// Wrapper over DB that implements many useful DB queries.
pub struct DbTool<'a, DB: Database> {
    pub(crate) db: &'a DB,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created.
    pub(crate) fn new(db: &'a DB) -> eyre::Result<Self> {
        Ok(Self { db })
    }

    /// Seeds the database with some random data, only used for testing
    pub fn seed(&mut self, len: u64) -> Result<()> {
        info!(target: "reth::cli", "Generating random block range from 0 to {len}");
        let chain = random_block_range(0..len, Default::default(), 0..64);

        self.db.update(|tx| {
            chain.into_iter().try_for_each(|block| {
                insert_canonical_block(tx, block, None, true)?;
                Ok::<_, eyre::Error>(())
            })
        })??;

        info!(target: "reth::cli", "Database seeded with {len} blocks");
        Ok(())
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [`HashMap`][std::collections::HashMap].
    pub fn list<T: Table>(
        &mut self,
        start: usize,
        len: usize,
    ) -> Result<BTreeMap<T::Key, T::Value>> {
        let data = self.db.view(|tx| {
            let mut cursor = tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

            // TODO: Upstream this in the DB trait.
            let start_walker = cursor.current().transpose();
            let walker = Walker::new(&mut cursor, start_walker);

            walker.skip(start).take(len).collect::<Vec<_>>()
        })?;

        data.into_iter()
            .collect::<Result<BTreeMap<T::Key, T::Value>, _>>()
            .map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database at the given path.
    pub fn drop(&mut self, path: &PlatformPath<DbPath>) -> Result<()> {
        info!(target: "reth::cli", "Dropping db at {}", path);
        std::fs::remove_dir_all(path).wrap_err("Dropping the database failed")?;
        Ok(())
    }

    /// Drops the provided table from the database.
    pub fn drop_table<T: Table>(&mut self) -> Result<()> {
        self.db.update(|tx| tx.clear::<T>())??;
        Ok(())
    }
}
