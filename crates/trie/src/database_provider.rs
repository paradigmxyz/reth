use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    table::{DupSort, Table},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_interfaces::db::DatabaseErrorInfo;
use reth_primitives::B256;

/// TODO:
#[derive(Clone, Debug)]
pub struct ConsistentDatabaseProvider<DB> {
    database: DB,
    tip: Option<B256>,
}

impl<DB: Database> ConsistentDatabaseProvider<DB> {
    /// Create new database provider.
    pub fn new(database: DB) -> Result<Self, DatabaseError> {
        let tip =
            database.tx()?.cursor_read::<tables::CanonicalHeaders>()?.last()?.map(|(_, hash)| hash);
        Ok(Self { database, tip })
    }

    /// Create new database provider with tip
    pub fn new_with_tip(database: DB, tip: Option<B256>) -> Self {
        Self { database, tip }
    }

    /// Opens new read transaction and performs a consistency check on the current tip.
    pub fn tx(&self) -> Result<DB::TX, DatabaseError> {
        let tx = self.database.tx()?;
        let tip = tx.cursor_read::<tables::CanonicalHeaders>()?.last()?.map(|(_, hash)| hash);
        if self.tip != tip {
            return Err(DatabaseError::InitTx(DatabaseErrorInfo {
                message: format!(
                    "tx has inconsistent view. expected: {:?}. got: {:?}",
                    self.tip, tip
                ),
                // Corresponds to `MDBX_PROBLEM`
                // <https://github.com/paradigmxyz/reth/blob/ada3547fd17fa9ff2aaf16449811cf5287f1c089/crates/storage/libmdbx-rs/mdbx-sys/libmdbx/mdbx.h#L1892>
                code: -30779,
            }))
        }
        Ok(tx)
    }
}
