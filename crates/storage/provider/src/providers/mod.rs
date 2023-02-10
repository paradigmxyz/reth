use crate::{BlockHashProvider, BlockProvider, Error, HeaderProvider, StateProviderFactory};
use reth_db::{
    database::{Database, DatabaseGAT},
    tables,
    transaction::DbTx,
};
use reth_interfaces::Result;
use reth_primitives::{rpc::BlockId, Block, BlockHash, BlockNumber, ChainInfo, Header, H256, U256};

mod state;
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
        self.db.view(|tx| tx.get::<tables::Headers>((0, *block_hash).into()))?.map_err(Into::into)
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        if let Some(hash) = self.db.view(|tx| tx.get::<tables::CanonicalHeaders>(num))?? {
            self.header(&hash)
        } else {
            Ok(None)
        }
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        if let Some(num) = self.db.view(|tx| tx.get::<tables::HeaderNumbers>(*hash))?? {
            let td = self.db.view(|tx| tx.get::<tables::HeaderTD>((num, *hash).into()))??;
            Ok(td.map(|v| v.0))
        } else {
            Ok(None)
        }
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
        Ok(ChainInfo {
            best_hash: Default::default(),
            best_number: 0,
            last_finalized: None,
            safe_finalized: None,
        })
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

        // check if block is canonical or not. Only canonical blocks have changesets.
        let canonical_block_hash = tx
            .get::<tables::CanonicalHeaders>(block_number)?
            .ok_or(Error::BlockCanonical { block_number, block_hash })?;
        if canonical_block_hash != block_hash {
            return Err(Error::NonCanonicalBlock {
                block_number,
                received_hash: block_hash,
                expected_hash: canonical_block_hash,
            }
            .into())
        }

        // get transition id
        let transition = tx
            .get::<tables::BlockTransitionIndex>(block_number)?
            .ok_or(Error::BlockTransition { block_number })?;

        Ok(HistoricalStateProvider::new(tx, transition))
    }
}

#[cfg(test)]
mod tests {
    use crate::StateProviderFactory;

    use super::ShareableDatabase;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};

    #[test]
    fn common_history_provider() {
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ShareableDatabase::new(db);
        let _ = provider.latest();
    }
}
