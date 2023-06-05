use crate::{
    traits::{BlockSource, ReceiptProvider},
    BlockHashProvider, BlockNumProvider, BlockProvider, EvmEnvProvider, HeaderProvider,
    ProviderError, StageCheckpointProvider, TransactionsProvider, WithdrawalsProvider,
};
use reth_db::{
    cursor::DbCursorRO,
    database::DatabaseGAT,
    models::StoredBlockBodyIndices,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::Result;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Block, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, ChainSpec, Head, Header, Receipt,
    SealedBlock, SealedHeader, TransactionMeta, TransactionSigned, TxHash, TxNumber, Withdrawal,
    H256, U256,
};
use reth_revm_primitives::{
    config::revm_spec,
    env::{fill_block_env, fill_cfg_and_block_env, fill_cfg_env},
    primitives::{BlockEnv, CfgEnv, SpecId},
};
use std::{ops::RangeBounds, sync::Arc};

/// A [`DatabaseProvider`] that holds a read-only database transaction.
pub(crate) type DatabaseProviderRO<'this, DB> =
    DatabaseProvider<'this, <DB as DatabaseGAT<'this>>::TX>;

/// A [`DatabaseProvider`] that holds a read-write database transaction.
pub(crate) type DatabaseProviderRW<'this, DB> =
    DatabaseProvider<'this, <DB as DatabaseGAT<'this>>::TXMut>;

/// A provider struct that fetchs data from the database.
/// Wrapper around [`DbTx`] and [`DbTxMut`]. Example: [`HeaderProvider`] [`BlockHashProvider`]
#[derive(Debug)]
pub struct DatabaseProvider<'this, TX>
where
    Self: 'this,
{
    /// Database transaction.
    tx: TX,
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
    _phantom_data: std::marker::PhantomData<&'this ()>,
}

impl<'this, TX: DbTxMut<'this>> DatabaseProvider<'this, TX> {
    /// Creates a provider with an inner read-write transaction.
    pub fn new_rw(tx: TX, chain_spec: Arc<ChainSpec>) -> Self {
        Self { tx, chain_spec, _phantom_data: std::marker::PhantomData }
    }
}

impl<'this, TX: DbTx<'this>> DatabaseProvider<'this, TX> {
    /// Creates a provider with an inner read-only transaction.
    pub fn new(tx: TX, chain_spec: Arc<ChainSpec>) -> Self {
        Self { tx, chain_spec, _phantom_data: std::marker::PhantomData }
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> TX {
        self.tx
    }
}

impl<'this, TX: DbTxMut<'this> + DbTx<'this>> DatabaseProvider<'this, TX> {
    /// Commit database transaction.
    pub fn commit(self) -> Result<bool> {
        Ok(self.tx.commit()?)
    }
}

impl<'this, TX: DbTx<'this>> HeaderProvider for DatabaseProvider<'this, TX> {
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        if let Some(num) = self.block_number(*block_hash)? {
            Ok(self.header_by_number(num)?)
        } else {
            Ok(None)
        }
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        Ok(self.tx.get::<tables::Headers>(num)?)
    }

    fn header_td(&self, block_hash: &BlockHash) -> Result<Option<U256>> {
        if let Some(num) = self.block_number(*block_hash)? {
            self.header_td_by_number(num)
        } else {
            Ok(None)
        }
    }

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        if let Some(td) = self.chain_spec.final_paris_difficulty(number) {
            // if this block is higher than the final paris(merge) block, return the final paris
            // difficulty
            return Ok(Some(td))
        }

        Ok(self.tx.get::<tables::HeaderTD>(number)?.map(|td| td.0))
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        let mut cursor = self.tx.cursor_read::<tables::Headers>()?;
        cursor
            .walk_range(range)?
            .map(|result| result.map(|(_, header)| header).map_err(Into::into))
            .collect::<Result<Vec<_>>>()
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader>> {
        let mut headers = vec![];
        for entry in self.tx.cursor_read::<tables::Headers>()?.walk_range(range)? {
            let (number, header) = entry?;
            let hash = self
                .block_hash(number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
            headers.push(header.seal(hash));
        }
        Ok(headers)
    }

    fn sealed_header(&self, number: BlockNumber) -> Result<Option<SealedHeader>> {
        if let Some(header) = self.header_by_number(number)? {
            let hash = self
                .block_hash(number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
            Ok(Some(header.seal(hash)))
        } else {
            Ok(None)
        }
    }
}

impl<'this, TX: DbTx<'this>> BlockHashProvider for DatabaseProvider<'this, TX> {
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        Ok(self.tx.get::<tables::CanonicalHeaders>(number)?)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        let range = start..end;
        let mut cursor = self.tx.cursor_read::<tables::CanonicalHeaders>()?;
        cursor
            .walk_range(range)?
            .map(|result| result.map(|(_, hash)| hash).map_err(Into::into))
            .collect::<Result<Vec<_>>>()
    }
}

impl<'this, TX: DbTx<'this>> BlockNumProvider for DatabaseProvider<'this, TX> {
    fn chain_info(&self) -> Result<ChainInfo> {
        let best_number = self.best_block_number()?;
        let best_hash = self.block_hash(best_number)?.unwrap_or_default();
        Ok(ChainInfo { best_hash, best_number })
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        Ok(self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or_default())
    }

    fn last_block_number(&self) -> Result<BlockNumber> {
        Ok(self.tx.cursor_read::<tables::CanonicalHeaders>()?.last()?.unwrap_or_default().0)
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::HeaderNumbers>(hash)?)
    }
}

impl<'this, TX: DbTx<'this>> BlockProvider for DatabaseProvider<'this, TX> {
    fn find_block_by_hash(&self, hash: H256, source: BlockSource) -> Result<Option<Block>> {
        if source.is_database() {
            self.block(hash.into())
        } else {
            Ok(None)
        }
    }

    fn block(&self, id: BlockHashOrNumber) -> Result<Option<Block>> {
        if let Some(number) = self.convert_hash_or_number(id)? {
            if let Some(header) = self.header_by_number(number)? {
                let withdrawals = self.withdrawals_by_block(number.into(), header.timestamp)?;
                let ommers = if withdrawals.is_none() { self.ommers(number.into())? } else { None }
                    .unwrap_or_default();
                let transactions = self
                    .transactions_by_block(number.into())?
                    .ok_or(ProviderError::BlockBodyIndicesNotFound(number))?;

                return Ok(Some(Block { header, body: transactions, ommers, withdrawals }))
            }
        }

        Ok(None)
    }

    fn pending_block(&self) -> Result<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_header(&self) -> Result<Option<SealedHeader>> {
        Ok(None)
    }

    fn ommers(&self, id: BlockHashOrNumber) -> Result<Option<Vec<Header>>> {
        if let Some(number) = self.convert_hash_or_number(id)? {
            // TODO: this can be optimized to return empty Vec post-merge
            let ommers = self.tx.get::<tables::BlockOmmers>(number)?.map(|o| o.ommers);
            return Ok(ommers)
        }

        Ok(None)
    }

    fn block_body_indices(&self, num: u64) -> Result<Option<StoredBlockBodyIndices>> {
        Ok(self.tx.get::<tables::BlockBodyIndices>(num)?)
    }
}

impl<'this, TX: DbTx<'this>> TransactionsProvider for DatabaseProvider<'this, TX> {
    fn transaction_id(&self, tx_hash: TxHash) -> Result<Option<TxNumber>> {
        Ok(self.tx.get::<tables::TxHashNumber>(tx_hash)?)
    }

    fn transaction_by_id(&self, id: TxNumber) -> Result<Option<TransactionSigned>> {
        Ok(self.tx.get::<tables::Transactions>(id)?.map(Into::into))
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        if let Some(id) = self.transaction_id(hash)? {
            Ok(self.transaction_by_id(id)?)
        } else {
            Ok(None)
        }
        .map(|tx| tx.map(Into::into))
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        if let Some(transaction_id) = self.transaction_id(tx_hash)? {
            if let Some(transaction) = self.transaction_by_id(transaction_id)? {
                let mut transaction_cursor = self.tx.cursor_read::<tables::TransactionBlock>()?;
                if let Some(block_number) =
                    transaction_cursor.seek(transaction_id).map(|b| b.map(|(_, bn)| bn))?
                {
                    if let Some(sealed_header) = self.sealed_header(block_number)? {
                        let (header, block_hash) = sealed_header.split();
                        if let Some(block_body) = self.block_body_indices(block_number)? {
                            // the index of the tx in the block is the offset:
                            // len([start..tx_id])
                            // SAFETY: `transaction_id` is always `>=` the block's first
                            // index
                            let index = transaction_id - block_body.first_tx_num();

                            let meta = TransactionMeta {
                                tx_hash,
                                index,
                                block_hash,
                                block_number,
                                base_fee: header.base_fee_per_gas,
                            };

                            return Ok(Some((transaction, meta)))
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    fn transaction_block(&self, id: TxNumber) -> Result<Option<BlockNumber>> {
        let mut cursor = self.tx.cursor_read::<tables::TransactionBlock>()?;
        Ok(cursor.seek(id)?.map(|(_, bn)| bn))
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> Result<Option<Vec<TransactionSigned>>> {
        if let Some(block_number) = self.convert_hash_or_number(id)? {
            if let Some(body) = self.block_body_indices(block_number)? {
                let tx_range = body.tx_num_range();
                return if tx_range.is_empty() {
                    Ok(Some(Vec::new()))
                } else {
                    let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;
                    let transactions = tx_cursor
                        .walk_range(tx_range)?
                        .map(|result| result.map(|(_, tx)| tx.into()))
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
        let mut results = Vec::default();
        let mut body_cursor = self.tx.cursor_read::<tables::BlockBodyIndices>()?;
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;
        for entry in body_cursor.walk_range(range)? {
            let (_, body) = entry?;
            let tx_num_range = body.tx_num_range();
            if tx_num_range.is_empty() {
                results.push(Vec::default());
            } else {
                results.push(
                    tx_cursor
                        .walk_range(tx_num_range)?
                        .map(|result| result.map(|(_, tx)| tx.into()))
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                );
            }
        }
        Ok(results)
    }
}

impl<'this, TX: DbTx<'this>> ReceiptProvider for DatabaseProvider<'this, TX> {
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>> {
        Ok(self.tx.get::<tables::Receipts>(id)?)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>> {
        if let Some(id) = self.transaction_id(hash)? {
            self.receipt(id)
        } else {
            Ok(None)
        }
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>> {
        if let Some(number) = self.convert_hash_or_number(block)? {
            if let Some(body) = self.block_body_indices(number)? {
                let tx_range = body.tx_num_range();
                return if tx_range.is_empty() {
                    Ok(Some(Vec::new()))
                } else {
                    let mut tx_cursor = self.tx.cursor_read::<tables::Receipts>()?;
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

impl<'this, TX: DbTx<'this>> WithdrawalsProvider for DatabaseProvider<'this, TX> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> Result<Option<Vec<Withdrawal>>> {
        if self.chain_spec.is_shanghai_activated_at_timestamp(timestamp) {
            if let Some(number) = self.convert_hash_or_number(id)? {
                // If we are past shanghai, then all blocks should have a withdrawal list, even if
                // empty
                let withdrawals = self
                    .tx
                    .get::<tables::BlockWithdrawals>(number)
                    .map(|w| w.map(|w| w.withdrawals))?
                    .unwrap_or_default();
                return Ok(Some(withdrawals))
            }
        }
        Ok(None)
    }

    fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> {
        let latest_block_withdrawal = self.tx.cursor_read::<tables::BlockWithdrawals>()?.last();
        latest_block_withdrawal
            .map(|block_withdrawal_pair| {
                block_withdrawal_pair
                    .and_then(|(_, block_withdrawal)| block_withdrawal.withdrawals.last().cloned())
            })
            .map_err(Into::into)
    }
}

impl<'this, TX: DbTx<'this>> EvmEnvProvider for DatabaseProvider<'this, TX> {
    fn fill_env_at(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> Result<()> {
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;
        self.fill_env_with_header(cfg, block_env, &header)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()> {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        fill_cfg_and_block_env(cfg, block_env, &self.chain_spec, header, total_difficulty);
        Ok(())
    }

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockHashOrNumber) -> Result<()> {
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;

        self.fill_block_env_with_header(block_env, &header)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
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
        fill_block_env(block_env, &self.chain_spec, header, after_merge);
        Ok(())
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockHashOrNumber) -> Result<()> {
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;
        self.fill_cfg_env_with_header(cfg, &header)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()> {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        fill_cfg_env(cfg, &self.chain_spec, header, total_difficulty);
        Ok(())
    }
}

impl<'this, TX: DbTx<'this>> StageCheckpointProvider for DatabaseProvider<'this, TX> {
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>> {
        Ok(self.tx.get::<tables::SyncStage>(id.to_string())?)
    }
}
