use crate::{HeaderProvider, ProviderImpl};
use reth_db::{database::Database, tables, transaction::DbTx};
use reth_interfaces::Result;
use reth_primitives::{BlockNumber, Header};

impl<DB: Database> HeaderProvider for ProviderImpl<DB> {
    fn header(&self, block_hash: &reth_primitives::BlockHash) -> Result<Option<Header>> {
        self.db.view(|tx| tx.get::<tables::Headers>((0, *block_hash).into()))?.map_err(Into::into)
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        if let Some(hash) = self.db.view(|tx| tx.get::<tables::CanonicalHeaders>(num))?? {
            self.header(&hash)
        } else {
            Ok(None)
        }
    }
}
