use crate::{
    BlockHashProvider, BlockIdProvider, BlockProvider, EvmEnvProvider, HeaderProvider,
    ProviderError, StateProviderFactory, TransactionsProvider, WithdrawalsProvider,
};
use reth_db::{
    cursor::DbCursorRO,
    database::{Database, DatabaseGAT},
    tables,
    transaction::DbTx,
};
use reth_interfaces::Result;
use reth_primitives::{
    Block, BlockHash, BlockId, BlockNumber, ChainInfo, ChainSpec, Hardfork, Head, Header, Receipt,
    TransactionSigned, TxHash, TxNumber, Withdrawal, H256, U256,
};
use reth_revm_primitives::{
    config::revm_spec,
    env::{fill_block_env, fill_cfg_and_block_env, fill_cfg_env},
};
use revm_primitives::{BlockEnv, CfgEnv, SpecId};
use std::{ops::RangeBounds, sync::Arc};

mod state;
use crate::traits::ReceiptProvider;
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
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
}

impl<DB> ShareableDatabase<DB> {
    /// create new database provider
    pub fn new(db: DB, chain_spec: ChainSpec) -> Self {
        Self { db, chain_spec: Arc::new(chain_spec) }
    }
}

impl<DB: Clone> Clone for ShareableDatabase<DB> {
    fn clone(&self) -> Self {
        Self { db: self.db.clone(), chain_spec: Arc::clone(&self.chain_spec) }
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
        Ok(self.db.view(|tx| tx.get::<tables::Headers>(num))??)
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

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        self.db.view(|tx| Ok(tx.get::<tables::HeaderTD>(number)?.map(|td| td.0)))?
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

impl<DB: Database> BlockIdProvider for ShareableDatabase<DB> {
    fn chain_info(&self) -> Result<ChainInfo> {
        let best_number = self
            .db
            .view(|tx| tx.get::<tables::SyncStage>("Finish".as_bytes().to_vec()))?
            .map_err(Into::<reth_interfaces::db::Error>::into)?
            .unwrap_or_default();
        let best_hash = self.block_hash(U256::from(best_number))?.unwrap_or_default();
        Ok(ChainInfo { best_hash, best_number, last_finalized: None, safe_finalized: None })
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.db.view(|tx| tx.get::<tables::HeaderNumbers>(hash))?.map_err(Into::into)
    }
}

impl<DB: Database> BlockProvider for ShareableDatabase<DB> {
    fn block(&self, id: BlockId) -> Result<Option<Block>> {
        if let Some(number) = self.block_number_for_id(id)? {
            if let Some(header) = self.header_by_number(number)? {
                let id = BlockId::Number(number.into());
                let tx = self.db.tx()?;
                let transactions =
                    self.transactions_by_block(id)?.ok_or(ProviderError::BlockBody { number })?;

                let ommers = tx.get::<tables::BlockOmmers>(header.number)?.map(|o| o.ommers);
                let withdrawals = self.withdrawals_by_block(id, header.timestamp)?;

                return Ok(Some(Block {
                    header,
                    body: transactions,
                    ommers: ommers.unwrap_or_default(),
                    withdrawals,
                }))
            }
        }

        Ok(None)
    }

    fn ommers(&self, id: BlockId) -> Result<Option<Vec<Header>>> {
        if let Some(number) = self.block_number_for_id(id)? {
            let tx = self.db.tx()?;
            // TODO: this can be optimized to return empty Vec post-merge
            let ommers = tx.get::<tables::BlockOmmers>(number)?.map(|o| o.ommers);
            return Ok(ommers)
        }

        Ok(None)
    }
}

impl<DB: Database> TransactionsProvider for ShareableDatabase<DB> {
    fn transaction_by_id(&self, id: TxNumber) -> Result<Option<TransactionSigned>> {
        self.db.view(|tx| tx.get::<tables::Transactions>(id))?.map_err(Into::into)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        self.db
            .view(|tx| {
                if let Some(id) = tx.get::<tables::TxHashNumber>(hash)? {
                    tx.get::<tables::Transactions>(id)
                } else {
                    Ok(None)
                }
            })?
            .map_err(Into::into)
    }

    fn transactions_by_block(&self, id: BlockId) -> Result<Option<Vec<TransactionSigned>>> {
        if let Some(number) = self.block_number_for_id(id)? {
            let tx = self.db.tx()?;
            if let Some(body) = tx.get::<tables::BlockBodies>(number)? {
                let tx_range = body.tx_id_range();
                return if tx_range.is_empty() {
                    Ok(Some(Vec::new()))
                } else {
                    let mut tx_cursor = tx.cursor_read::<tables::Transactions>()?;
                    let transactions = tx_cursor
                        .walk_range(tx_range)?
                        .map(|result| result.map(|(_, tx)| tx))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok(Some(transactions))
                }
            }
        }
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        let tx = self.db.tx()?;
        let mut results = Vec::default();
        let mut body_cursor = tx.cursor_read::<tables::BlockBodies>()?;
        let mut tx_cursor = tx.cursor_read::<tables::Transactions>()?;
        for entry in body_cursor.walk_range(range)? {
            let (_, body) = entry?;
            let tx_range = body.tx_id_range();
            if body.tx_id_range().is_empty() {
                results.push(Vec::default());
            } else {
                results.push(
                    tx_cursor
                        .walk_range(tx_range)?
                        .map(|result| result.map(|(_, tx)| tx))
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                );
            }
        }
        Ok(results)
    }
}

impl<DB: Database> ReceiptProvider for ShareableDatabase<DB> {
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>> {
        self.db.view(|tx| tx.get::<tables::Receipts>(id))?.map_err(Into::into)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>> {
        self.db
            .view(|tx| {
                if let Some(id) = tx.get::<tables::TxHashNumber>(hash)? {
                    tx.get::<tables::Receipts>(id)
                } else {
                    Ok(None)
                }
            })?
            .map_err(Into::into)
    }

    fn receipts_by_block(&self, block: BlockId) -> Result<Option<Vec<Receipt>>> {
        if let Some(number) = self.block_number_for_id(block)? {
            let tx = self.db.tx()?;
            if let Some(body) = tx.get::<tables::BlockBodies>(number)? {
                let tx_range = body.tx_id_range();
                return if tx_range.is_empty() {
                    Ok(Some(Vec::new()))
                } else {
                    let mut tx_cursor = tx.cursor_read::<tables::Receipts>()?;
                    let transactions = tx_cursor
                        .walk_range(tx_range)?
                        .map(|result| result.map(|(_, tx)| tx))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok(Some(transactions))
                }
            }
        }
        Ok(None)
    }
}

impl<DB: Database> WithdrawalsProvider for ShareableDatabase<DB> {
    fn withdrawals_by_block(&self, id: BlockId, timestamp: u64) -> Result<Option<Vec<Withdrawal>>> {
        if self.chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(timestamp) {
            if let Some(number) = self.block_number_for_id(id)? {
                return Ok(self
                    .db
                    .view(|tx| tx.get::<tables::BlockWithdrawals>(number))??
                    .map(|w| w.withdrawals))
            }
        }
        Ok(None)
    }
}

impl<DB: Database> EvmEnvProvider for ShareableDatabase<DB> {
    fn fill_env_at(&self, cfg: &mut CfgEnv, block_env: &mut BlockEnv, at: BlockId) -> Result<()> {
        let hash = self.block_hash_for_id(at)?.ok_or(ProviderError::HeaderNotFound)?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound)?;
        self.fill_env_with_header(cfg, block_env, &header)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()> {
        let total_difficulty =
            self.header_td_by_number(header.number)?.ok_or(ProviderError::HeaderNotFound)?;
        fill_cfg_and_block_env(cfg, block_env, &self.chain_spec, header, total_difficulty);
        Ok(())
    }

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockId) -> Result<()> {
        let hash = self.block_hash_for_id(at)?.ok_or(ProviderError::HeaderNotFound)?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound)?;

        self.fill_block_env_with_header(block_env, &header)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        let total_difficulty =
            self.header_td_by_number(header.number)?.ok_or(ProviderError::HeaderNotFound)?;
        let spec_id = revm_spec(
            &self.chain_spec,
            Head {
                number: header.number,
                timestamp: header.timestamp,
                difficulty: header.difficulty,
                total_difficulty,
                // Not required
                hash: Default::default(),
            },
        );
        let after_merge = spec_id >= SpecId::MERGE;
        fill_block_env(block_env, header, after_merge);
        Ok(())
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockId) -> Result<()> {
        let hash = self.block_hash_for_id(at)?.ok_or(ProviderError::HeaderNotFound)?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound)?;
        self.fill_cfg_env_with_header(cfg, &header)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()> {
        let total_difficulty =
            self.header_td_by_number(header.number)?.ok_or(ProviderError::HeaderNotFound)?;
        fill_cfg_env(cfg, &self.chain_spec, header, total_difficulty);
        Ok(())
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
            .ok_or(ProviderError::BlockTransition { block_number })?;

        Ok(HistoricalStateProvider::new(tx, transition))
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<Self::HistorySP<'_>> {
        let tx = self.db.tx()?;
        // get block number
        let block_number = tx
            .get::<tables::HeaderNumbers>(block_hash)?
            .ok_or(ProviderError::BlockHash { block_hash })?;

        // get transition id
        let transition = tx
            .get::<tables::BlockTransitionIndex>(block_number)?
            .ok_or(ProviderError::BlockTransition { block_number })?;

        Ok(HistoricalStateProvider::new(tx, transition))
    }
}

#[cfg(test)]
mod tests {
    use super::ShareableDatabase;
    use crate::{BlockIdProvider, StateProviderFactory};
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_primitives::{ChainSpecBuilder, H256};

    #[test]
    fn common_history_provider() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ShareableDatabase::new(db, chain_spec);
        let _ = provider.latest();
    }

    #[test]
    fn default_chain_info() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ShareableDatabase::new(db, chain_spec);

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, H256::zero());
        assert_eq!(chain_info.last_finalized, None);
        assert_eq!(chain_info.safe_finalized, None);
    }
}
