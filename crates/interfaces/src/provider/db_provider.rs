use crate::{
    db::{tables, Database, DbTx},
    provider::HeaderProvider,
};

/// Provider
pub struct DbProvider<DB: Database> {
    /// Database
    db: DB,
}

impl<DB: Database> DbProvider<DB> {
    /// create new database provider
    pub fn new(db: DB) -> Self {
        Self { db }
    }
}

impl<DB: Database> HeaderProvider for DbProvider<DB> {
    fn header(
        &self,
        block_hash: &reth_primitives::BlockHash,
    ) -> crate::Result<Option<reth_primitives::Header>> {
        self.db.view(|tx| tx.get::<tables::Headers>((0, *block_hash).into()))?.map_err(Into::into)
    }

    fn is_known(&self, block_hash: &reth_primitives::BlockHash) -> crate::Result<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }
}
