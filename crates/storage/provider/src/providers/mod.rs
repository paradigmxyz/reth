use crate::{BlockHashProvider, BlockProvider, Error, HeaderProvider, StateProviderFactory};
use reth_db::{
    database::{Database, DatabaseGAT},
    tables,
    transaction::DbTx,
};
use reth_interfaces::Result;
use reth_primitives::{rpc::BlockId, Block, BlockHash, BlockNumber, ChainInfo, Header, H256, U256};
use std::ops::RangeBounds;

mod state;
use reth_db::cursor::DbCursorRO;
pub use state::{
    chain::ChainState,
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};

/// A common provider that fetches data from a database.
///
/// This provider implements most provider or provider factory traits.
pub struct ShareableDatabase<DB> {
    /// Database
    db: DB,
}

impl<DB> ShareableDatabase<DB> {
    /// create new database provider
    pub fn new(db: DB) -> Self {
        Self { db }
    }
}

impl<DB: Clone> Clone for ShareableDatabase<DB> {
    fn clone(&self) -> Self {
        Self { db: self.db.clone() }
    }
}

impl<DB: Database> HeaderProvider for ShareableDatabase<DB> {
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        self.db.view(|tx| {
            if let Some(num) = tx.get::<tables::HeaderNumbers>(*block_hash)? {
                Ok(tx.get::<tables::Headers>(num)?)
            } else {
                Ok(None)
            }
        })?
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        if let Some(hash) = self.db.view(|tx| tx.get::<tables::CanonicalHeaders>(num))?? {
            self.header(&hash)
        } else {
            Ok(None)
        }
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        self.db.view(|tx| {
            if let Some(num) = tx.get::<tables::HeaderNumbers>(*hash)? {
                Ok(tx.get::<tables::HeaderTD>(num)?.map(|td| td.0))
            } else {
                Ok(None)
            }
        })?
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        self.db
            .view(|tx| {
                let mut cursor = tx.cursor_read::<tables::Headers>()?;
                cursor
                    .walk_range(range)?
                    .map(|result| result.map(|(_, header)| header).map_err(Into::into))
                    .collect::<Result<Vec<_>>>()
            })?
            .map_err(Into::into)
    }
}

impl<DB: Database> BlockHashProvider for ShareableDatabase<DB> {
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        // TODO: This unwrap is potentially unsafe
        self.db
            .view(|tx| tx.get::<tables::CanonicalHeaders>(number.try_into().unwrap()))?
            .map_err(Into::into)
    }
}

impl<DB: Database> BlockProvider for ShareableDatabase<DB> {
    fn chain_info(&self) -> Result<ChainInfo> {
        let best_number = self
            .db
            .view(|tx| tx.get::<tables::SyncStage>("Finish".as_bytes().to_vec()))?
            .map_err(Into::<reth_interfaces::db::Error>::into)?
            .unwrap_or_default();
        let best_hash = self.block_hash(U256::from(best_number))?.unwrap_or_default();
        Ok(ChainInfo { best_hash, best_number, last_finalized: None, safe_finalized: None })
    }

    fn block(&self, _id: BlockId) -> Result<Option<Block>> {
        // TODO
        Ok(None)
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.db.view(|tx| tx.get::<tables::HeaderNumbers>(hash))?.map_err(Into::into)
    }
}

impl<DB: Database> StateProviderFactory for ShareableDatabase<DB> {
    type HistorySP<'a> = HistoricalStateProvider<'a,<DB as DatabaseGAT<'a>>::TX> where Self: 'a;
    type LatestSP<'a> = LatestStateProvider<'a,<DB as DatabaseGAT<'a>>::TX> where Self: 'a;
    /// Storage provider for latest block
    fn latest(&self) -> Result<Self::LatestSP<'_>> {
        Ok(LatestStateProvider::new(self.db.tx()?))
    }

    fn history_by_block_number(&self, block_number: BlockNumber) -> Result<Self::HistorySP<'_>> {
        let tx = self.db.tx()?;

        // get transition id
        let transition = tx
            .get::<tables::BlockTransitionIndex>(block_number)?
            .ok_or(Error::BlockTransition { block_number })?;

        Ok(HistoricalStateProvider::new(tx, transition))
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<Self::HistorySP<'_>> {
        let tx = self.db.tx()?;
        // get block number
        let block_number =
            tx.get::<tables::HeaderNumbers>(block_hash)?.ok_or(Error::BlockHash { block_hash })?;

        // get transition id
        let transition = tx
            .get::<tables::BlockTransitionIndex>(block_number)?
            .ok_or(Error::BlockTransition { block_number })?;

        Ok(HistoricalStateProvider::new(tx, transition))
    }
}

#[cfg(test)]
mod tests {
    use crate::{BlockProvider, StateProviderFactory};

    use super::ShareableDatabase;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_primitives::H256;

    #[test]
    fn common_history_provider() {
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ShareableDatabase::new(db);
        let _ = provider.latest();
    }

    #[test]
    fn default_chain_info() {
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ShareableDatabase::new(db);

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, H256::zero());
        assert_eq!(chain_info.last_finalized, None);
        assert_eq!(chain_info.safe_finalized, None);
    }
}
