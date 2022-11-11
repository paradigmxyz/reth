use crate::{
    db::{tables, Database, DbTx},
    provider::{HeaderProvider, ProviderImpl},
};

impl<DB: Database> HeaderProvider for ProviderImpl<DB> {
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
