use crate::{
    providers::state::{historical::HistoricalStateProvider, latest::LatestStateProvider},
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, ChainSpecProvider, EvmEnvProvider,
    HeaderProvider, ProviderError, PruneCheckpointReader, StageCheckpointReader, StateProviderBox,
    TransactionsProvider, WithdrawalsProvider,
};
use reth_db::{database::Database, init_db, models::StoredBlockBodyIndices, DatabaseEnv};
use reth_interfaces::Result;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Address, Block, BlockHash, BlockHashOrNumber, BlockNumber, BlockWithSenders, ChainInfo,
    ChainSpec, Header, PruneCheckpoint, PrunePart, Receipt, SealedBlock, SealedHeader,
    TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, Withdrawal,
    H256, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
use std::{ops::RangeBounds, sync::Arc};
use tracing::trace;

mod provider;
pub use provider::{DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW};
use reth_interfaces::db::LogLevel;

/// A common provider that fetches data from a database.
///
/// This provider implements most provider or provider factory traits.
#[derive(Debug)]
pub struct ProviderFactory<DB> {
    /// Database
    db: DB,
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
}

impl<DB: Database> ProviderFactory<DB> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    pub fn provider(&self) -> Result<DatabaseProviderRO<'_, DB>> {
        Ok(DatabaseProvider::new(self.db.tx()?, self.chain_spec.clone()))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    pub fn provider_rw(&self) -> Result<DatabaseProviderRW<'_, DB>> {
        Ok(DatabaseProviderRW(DatabaseProvider::new_rw(self.db.tx_mut()?, self.chain_spec.clone())))
    }
}

impl<DB> ProviderFactory<DB> {
    /// create new database provider
    pub fn new(db: DB, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, chain_spec }
    }
}

impl<DB: Database> ProviderFactory<DB> {
    /// create new database provider by passing a path. [`ProviderFactory`] will own the database
    /// instance.
    pub fn new_with_database_path<P: AsRef<std::path::Path>>(
        path: P,
        chain_spec: Arc<ChainSpec>,
        log_level: Option<LogLevel>,
    ) -> Result<ProviderFactory<DatabaseEnv>> {
        Ok(ProviderFactory::<DatabaseEnv> {
            db: init_db(path, log_level)
                .map_err(|e| reth_interfaces::Error::Custom(e.to_string()))?,
            chain_spec,
        })
    }
}

impl<DB: Clone> Clone for ProviderFactory<DB> {
    fn clone(&self) -> Self {
        Self { db: self.db.clone(), chain_spec: Arc::clone(&self.chain_spec) }
    }
}

impl<DB: Database> ProviderFactory<DB> {
    /// Storage provider for latest block
    pub fn latest(&self) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.db.tx()?)))
    }

    /// Storage provider for state at that given block
    fn state_provider_by_block_number(
        &self,
        mut block_number: BlockNumber,
    ) -> Result<StateProviderBox<'_>> {
        let provider = self.provider()?;

        if block_number == provider.best_block_number().unwrap_or_default() &&
            block_number == provider.last_block_number().unwrap_or_default()
        {
            return Ok(Box::new(LatestStateProvider::new(provider.into_tx())))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        block_number += 1;

        let account_history_prune_checkpoint =
            provider.get_prune_checkpoint(PrunePart::AccountHistory)?;
        let storage_history_prune_checkpoint =
            provider.get_prune_checkpoint(PrunePart::StorageHistory)?;

        let mut state_provider = HistoricalStateProvider::new(provider.into_tx(), block_number);

        // If we pruned account or storage history, we can't return state on every historical block.
        // Instead, we should cap it at the latest prune checkpoint for corresponding prune part.
        if let Some(prune_checkpoint_block_number) =
            account_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_account_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }
        if let Some(prune_checkpoint_block_number) =
            storage_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_storage_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }

        Ok(Box::new(state_provider))
    }

    /// Storage provider for state at that given block
    pub fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> Result<StateProviderBox<'_>> {
        let state_provider = self.state_provider_by_block_number(block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block number");
        Ok(state_provider)
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<StateProviderBox<'_>> {
        let block_number = self
            .provider()?
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        let state_provider = self.state_provider_by_block_number(block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block hash");
        Ok(state_provider)
    }
}

impl<DB: Database> HeaderProvider for ProviderFactory<DB> {
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        self.provider()?.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        self.provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        self.provider()?.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        self.provider()?.headers_range(range)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader>> {
        self.provider()?.sealed_headers_range(range)
    }

    fn sealed_header(&self, number: BlockNumber) -> Result<Option<SealedHeader>> {
        self.provider()?.sealed_header(number)
    }
}

impl<DB: Database> BlockHashReader for ProviderFactory<DB> {
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        self.provider()?.block_hash(number)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        self.provider()?.canonical_hashes_range(start, end)
    }
}

impl<DB: Database> BlockNumReader for ProviderFactory<DB> {
    fn chain_info(&self) -> Result<ChainInfo> {
        self.provider()?.chain_info()
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        self.provider()?.best_block_number()
    }

    fn last_block_number(&self) -> Result<BlockNumber> {
        self.provider()?.last_block_number()
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.provider()?.block_number(hash)
    }
}

impl<DB: Database> BlockReader for ProviderFactory<DB> {
    fn find_block_by_hash(&self, hash: H256, source: BlockSource) -> Result<Option<Block>> {
        self.provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> Result<Option<Block>> {
        self.provider()?.block(id)
    }

    fn pending_block(&self) -> Result<Option<SealedBlock>> {
        self.provider()?.pending_block()
    }

    fn pending_block_and_receipts(&self) -> Result<Option<(SealedBlock, Vec<Receipt>)>> {
        self.provider()?.pending_block_and_receipts()
    }

    fn ommers(&self, id: BlockHashOrNumber) -> Result<Option<Vec<Header>>> {
        self.provider()?.ommers(id)
    }

    fn block_body_indices(&self, number: BlockNumber) -> Result<Option<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices(number)
    }

    fn block_with_senders(&self, number: BlockNumber) -> Result<Option<BlockWithSenders>> {
        self.provider()?.block_with_senders(number)
    }
}

impl<DB: Database> TransactionsProvider for ProviderFactory<DB> {
    fn transaction_id(&self, tx_hash: TxHash) -> Result<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> Result<Option<TransactionSigned>> {
        self.provider()?.transaction_by_id(id)
    }

    fn transaction_by_id_no_hash(&self, id: TxNumber) -> Result<Option<TransactionSignedNoHash>> {
        self.provider()?.transaction_by_id_no_hash(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        self.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        self.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> Result<Option<BlockNumber>> {
        self.provider()?.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> Result<Option<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<TransactionSignedNoHash>> {
        self.provider()?.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(&self, range: impl RangeBounds<TxNumber>) -> Result<Vec<Address>> {
        self.provider()?.senders_by_tx_range(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> Result<Option<Address>> {
        self.provider()?.transaction_sender(id)
    }
}

impl<DB: Database> ReceiptProvider for ProviderFactory<DB> {
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>> {
        self.provider()?.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>> {
        self.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>> {
        self.provider()?.receipts_by_block(block)
    }
}

impl<DB: Database> WithdrawalsProvider for ProviderFactory<DB> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> Result<Option<Vec<Withdrawal>>> {
        self.provider()?.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> {
        self.provider()?.latest_withdrawal()
    }
}

impl<DB: Database> StageCheckpointReader for ProviderFactory<DB> {
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>> {
        self.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> Result<Option<Vec<u8>>> {
        self.provider()?.get_stage_checkpoint_progress(id)
    }
}

impl<DB: Database> EvmEnvProvider for ProviderFactory<DB> {
    fn fill_env_at(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> Result<()> {
        self.provider()?.fill_env_at(cfg, block_env, at)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()> {
        self.provider()?.fill_env_with_header(cfg, block_env, header)
    }

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockHashOrNumber) -> Result<()> {
        self.provider()?.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        self.provider()?.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockHashOrNumber) -> Result<()> {
        self.provider()?.fill_cfg_env_at(cfg, at)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()> {
        self.provider()?.fill_cfg_env_with_header(cfg, header)
    }
}

impl<DB> ChainSpecProvider for ProviderFactory<DB>
where
    DB: Send + Sync,
{
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<DB: Database> PruneCheckpointReader for ProviderFactory<DB> {
    fn get_prune_checkpoint(&self, part: PrunePart) -> Result<Option<PruneCheckpoint>> {
        self.provider()?.get_prune_checkpoint(part)
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderFactory;
    use crate::{BlockHashReader, BlockNumReader, BlockWriter, TransactionsProvider};
    use assert_matches::assert_matches;
    use reth_db::{
        tables,
        test_utils::{create_test_rw_db, ERROR_TEMPDIR},
        DatabaseEnv,
    };
    use reth_interfaces::test_utils::{generators, generators::random_block};
    use reth_primitives::{
        hex_literal::hex, ChainSpecBuilder, PruneMode, PruneModes, SealedBlock, TxNumber, H256,
    };
    use reth_rlp::Decodable;
    use std::{ops::RangeInclusive, sync::Arc};

    #[test]
    fn common_history_provider() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_rw_db();
        let provider = ProviderFactory::new(db, Arc::new(chain_spec));
        let _ = provider.latest();
    }

    #[test]
    fn default_chain_info() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_rw_db();
        let factory = ProviderFactory::new(db, Arc::new(chain_spec));
        let provider = factory.provider().unwrap();

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, H256::zero());
    }

    #[test]
    fn provider_flow() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_rw_db();
        let factory = ProviderFactory::new(db, Arc::new(chain_spec));
        let provider = factory.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }

    #[test]
    fn provider_factory_with_database_path() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let factory = ProviderFactory::<DatabaseEnv>::new_with_database_path(
            tempfile::TempDir::new().expect(ERROR_TEMPDIR).into_path(),
            Arc::new(chain_spec),
            None,
        )
        .unwrap();

        let provider = factory.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }

    #[test]
    fn insert_block_with_prune_modes() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_rw_db();
        let factory = ProviderFactory::new(db, Arc::new(chain_spec));

        let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();

        {
            let provider = factory.provider_rw().unwrap();
            assert_matches!(provider.insert_block(block.clone(), None, None), Ok(_));
            assert_matches!(
                provider.transaction_sender(0), Ok(Some(sender))
                if sender == block.body[0].recover_signer().unwrap()
            );
            assert_matches!(provider.transaction_id(block.body[0].hash), Ok(Some(0)));
        }

        {
            let provider = factory.provider_rw().unwrap();
            assert_matches!(
                provider.insert_block(
                    block.clone(),
                    None,
                    Some(&PruneModes {
                        sender_recovery: Some(PruneMode::Full),
                        transaction_lookup: Some(PruneMode::Full),
                        ..PruneModes::none()
                    })
                ),
                Ok(_)
            );
            assert_matches!(provider.transaction_sender(0), Ok(None));
            assert_matches!(provider.transaction_id(block.body[0].hash), Ok(None));
        }
    }

    #[test]
    fn get_take_block_transaction_range_recover_senders() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_rw_db();
        let factory = ProviderFactory::new(db, Arc::new(chain_spec));

        let mut rng = generators::rng();
        let block = random_block(&mut rng, 0, None, Some(3), None);

        let tx_ranges: Vec<RangeInclusive<TxNumber>> = vec![0..=0, 1..=1, 2..=2, 0..=1, 1..=2];
        for range in tx_ranges {
            let provider = factory.provider_rw().unwrap();

            assert_matches!(provider.insert_block(block.clone(), None, None), Ok(_));

            let senders = provider.get_or_take::<tables::TxSenders, true>(range.clone());
            assert_eq!(
                senders,
                Ok(range
                    .clone()
                    .map(|tx_number| (
                        tx_number,
                        block.body[tx_number as usize].recover_signer().unwrap()
                    ))
                    .collect())
            );

            let db_senders = provider.senders_by_tx_range(range);
            assert_eq!(db_senders, Ok(vec![]));

            let result = provider.get_take_block_transaction_range::<true>(0..=0);
            assert_eq!(
                result,
                Ok(vec![(
                    0,
                    block.body.iter().cloned().map(|tx| tx.into_ecrecovered().unwrap()).collect()
                )])
            )
        }
    }
}
