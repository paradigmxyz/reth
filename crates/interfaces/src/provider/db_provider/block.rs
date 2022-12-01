use crate::{
    db::{tables, Database, DbTx},
    provider::{HeaderProvider, ProviderImpl},
};
use reth_primitives::{BlockNumber, Header};

impl<DB: Database> HeaderProvider for ProviderImpl<DB> {
    fn header(&self, block_hash: &reth_primitives::BlockHash) -> crate::Result<Option<Header>> {
        self.db.view(|tx| tx.get::<tables::Headers>((0, *block_hash).into()))?.map_err(Into::into)
    }

    fn header_by_number(&self, num: BlockNumber) -> crate::Result<Option<Header>> {
        if let Some(hash) = self.db.view(|tx| tx.get::<tables::CanonicalHeaders>(num))?? {
            self.header(&hash)
        } else {
            Ok(None)
        }
    }
}
