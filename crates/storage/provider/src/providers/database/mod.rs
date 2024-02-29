use crate::{
    providers::{
        state::{historical::HistoricalStateProvider, latest::LatestStateProvider},
        SnapshotProvider,
    },
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, ChainSpecProvider, EvmEnvProvider,
    HeaderProvider, HeaderSyncGap, HeaderSyncGapProvider, HeaderSyncMode, ProviderError,
    PruneCheckpointReader, StageCheckpointReader, StateProviderBox, TransactionVariant,
    TransactionsProvider, WithdrawalsProvider,
};
use reth_db::{database::Database, init_db, models::StoredBlockBodyIndices, DatabaseEnv};
use reth_interfaces::{provider::ProviderResult, RethError, RethResult};
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{
    snapshot::HighestSnapshots,
    stage::{StageCheckpoint, StageId},
    Address, Block, BlockHash, BlockHashOrNumber, BlockNumber, BlockWithSenders, ChainInfo,
    ChainSpec, Header, PruneCheckpoint, PruneSegment, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader, TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber,
    Withdrawal, Withdrawals, B256, U256,
};
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use std::{
    ops::{RangeBounds, RangeInclusive},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;
use tracing::trace;

mod metrics;
mod provider;

pub use provider::{DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW};
use reth_db::mdbx::DatabaseArguments;

/// A common provider that fetches data from a database.
///
/// This provider implements most provider or provider factory traits.
#[derive(Debug)]
pub struct ProviderFactory<DB> {
    /// Database
    db: DB,
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
    /// Snapshot Provider
    snapshot_provider: Option<Arc<SnapshotProvider>>,
}

impl<DB: Clone> Clone for ProviderFactory<DB> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            chain_spec: Arc::clone(&self.chain_spec),
            snapshot_provider: self.snapshot_provider.clone(),
        }
    }
}

impl<DB> ProviderFactory<DB> {
    /// Create new database provider factory.
    pub fn new(db: DB, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, chain_spec, snapshot_provider: None }
    }

    /// Create new database provider by passing a path. [`ProviderFactory`] will own the database
    /// instance.
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        chain_spec: Arc<ChainSpec>,
        args: DatabaseArguments,
    ) -> RethResult<ProviderFactory<DatabaseEnv>> {
        Ok(ProviderFactory::<DatabaseEnv> {
            db: init_db(path, args).map_err(|e| RethError::Custom(e.to_string()))?,
            chain_spec,
            snapshot_provider: None,
        })
    }

    /// Database provider that comes with a shared snapshot provider.
    pub fn with_snapshots(
        mut self,
        snapshots_path: PathBuf,
        highest_snapshot_tracker: watch::Receiver<Option<HighestSnapshots>>,
    ) -> ProviderResult<Self> {
        self.snapshot_provider = Some(Arc::new(
            SnapshotProvider::new(snapshots_path)?
                .with_highest_tracker(Some(highest_snapshot_tracker)),
        ));
        Ok(self)
    }

    /// Returns reference to the underlying database.
    pub fn db_ref(&self) -> &DB {
        &self.db
    }
}

impl<DB: Database> ProviderFactory<DB> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    #[track_caller]
    pub fn provider(&self) -> ProviderResult<DatabaseProviderRO<DB>> {
        let mut provider = DatabaseProvider::new(self.db.tx()?, self.chain_spec.clone());

        if let Some(snapshot_provider) = &self.snapshot_provider {
            provider = provider.with_snapshot_provider(snapshot_provider.clone());
        }

        Ok(provider)
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    #[track_caller]
    pub fn provider_rw(&self) -> ProviderResult<DatabaseProviderRW<DB>> {
        let mut provider = DatabaseProvider::new_rw(self.db.tx_mut()?, self.chain_spec.clone());

        if let Some(snapshot_provider) = &self.snapshot_provider {
            provider = provider.with_snapshot_provider(snapshot_provider.clone());
        }

        Ok(DatabaseProviderRW(provider))
    }

    /// Storage provider for latest block
    #[track_caller]
    pub fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.db.tx()?)))
    }

    /// Storage provider for state at that given block
    fn state_provider_by_block_number(
        &self,
        provider: DatabaseProviderRO<DB>,
        mut block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        if block_number == provider.best_block_number().unwrap_or_default() &&
            block_number == provider.last_block_number().unwrap_or_default()
        {
            return Ok(Box::new(LatestStateProvider::new(provider.into_tx())))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        block_number += 1;

        let account_history_prune_checkpoint =
            provider.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let storage_history_prune_checkpoint =
            provider.get_prune_checkpoint(PruneSegment::StorageHistory)?;

        let mut state_provider = HistoricalStateProvider::new(provider.into_tx(), block_number);

        // If we pruned account or storage history, we can't return state on every historical block.
        // Instead, we should cap it at the latest prune checkpoint for corresponding prune segment.
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
    ) -> ProviderResult<StateProviderBox> {
        let provider = self.provider()?;
        let state_provider = self.state_provider_by_block_number(provider, block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block number");
        Ok(state_provider)
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        let provider = self.provider()?;

        let block_number = provider
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        let state_provider = self.state_provider_by_block_number(provider, block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block hash");
        Ok(state_provider)
    }
}

impl<DB: Database> HeaderSyncGapProvider for ProviderFactory<DB> {
    fn sync_gap(
        &self,
        mode: HeaderSyncMode,
        highest_uninterrupted_block: BlockNumber,
    ) -> RethResult<HeaderSyncGap> {
        self.provider()?.sync_gap(mode, highest_uninterrupted_block)
    }
}

impl<DB: Database> HeaderProvider for ProviderFactory<DB> {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.provider()?.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        self.provider()?.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.provider()?.headers_range(range)
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.provider()?.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.provider()?.sealed_headers_range(range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.provider()?.sealed_headers_while(range, predicate)
    }
}

impl<DB: Database> BlockHashReader for ProviderFactory<DB> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.provider()?.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.provider()?.canonical_hashes_range(start, end)
    }
}

impl<DB: Database> BlockNumReader for ProviderFactory<DB> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        self.provider()?.chain_info()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        self.provider()?.best_block_number()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.provider()?.last_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.block_number(hash)
    }
}

impl<DB: Database> BlockReader for ProviderFactory<DB> {
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        self.provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        self.provider()?.block(id)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        self.provider()?.pending_block()
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.provider()?.pending_block_with_senders()
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        self.provider()?.pending_block_and_receipts()
    }

    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        self.provider()?.ommers(id)
    }

    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices(number)
    }

    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        self.provider()?.block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        self.provider()?.block_range(range)
    }
}

impl<DB: Database> TransactionsProvider for ProviderFactory<DB> {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.provider()?.transaction_by_id(id)
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.provider()?.transaction_by_id_no_hash(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        self.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.provider()?.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.provider()?.senders_by_tx_range(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.provider()?.transaction_sender(id)
    }
}

impl<DB: Database> ReceiptProvider for ProviderFactory<DB> {
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.provider()?.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        self.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        self.provider()?.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.provider()?.receipts_by_tx_range(range)
    }
}

impl<DB: Database> WithdrawalsProvider for ProviderFactory<DB> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        self.provider()?.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        self.provider()?.latest_withdrawal()
    }
}

impl<DB: Database> StageCheckpointReader for ProviderFactory<DB> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.provider()?.get_stage_checkpoint_progress(id)
    }
}

impl<DB: Database> EvmEnvProvider for ProviderFactory<DB> {
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_env_at(cfg, block_env, at, evm_config)
    }

    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_env_with_header(cfg, block_env, header, evm_config)
    }

    fn fill_block_env_at(
        &self,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> ProviderResult<()> {
        self.provider()?.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(
        &self,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> ProviderResult<()> {
        self.provider()?.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_cfg_env_at(cfg, at, evm_config)
    }

    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.provider()?.fill_cfg_env_with_header(cfg, header, evm_config)
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
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.provider()?.get_prune_checkpoint(segment)
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderFactory;
    use crate::{
        test_utils::create_test_provider_factory, BlockHashReader, BlockNumReader, BlockWriter,
        HeaderSyncGapProvider, HeaderSyncMode, TransactionsProvider,
    };
    use alloy_rlp::Decodable;
    use assert_matches::assert_matches;
    use rand::Rng;
    use reth_db::{tables, test_utils::ERROR_TEMPDIR, transaction::DbTxMut, DatabaseEnv};
    use reth_interfaces::{
        provider::ProviderError,
        test_utils::{
            generators,
            generators::{random_block, random_header},
        },
        RethError,
    };
    use reth_primitives::{
        hex_literal::hex, ChainSpecBuilder, PruneMode, PruneModes, SealedBlock, TxNumber, B256,
    };
    use std::{ops::RangeInclusive, sync::Arc};
    use tokio::sync::watch;

    #[test]
    fn common_history_provider() {
        let factory = create_test_provider_factory();
        let _ = factory.latest();
    }

    #[test]
    fn default_chain_info() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, B256::ZERO);
    }

    #[test]
    fn provider_flow() {
        let factory = create_test_provider_factory();
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
            Default::default(),
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
        let factory = create_test_provider_factory();

        let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();

        {
            let provider = factory.provider_rw().unwrap();
            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap(), None),
                Ok(_)
            );
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
                    block.clone().try_seal_with_senders().unwrap(),
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
        let factory = create_test_provider_factory();

        let mut rng = generators::rng();
        let block = random_block(&mut rng, 0, None, Some(3), None);

        let tx_ranges: Vec<RangeInclusive<TxNumber>> = vec![0..=0, 1..=1, 2..=2, 0..=1, 1..=2];
        for range in tx_ranges {
            let provider = factory.provider_rw().unwrap();

            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap(), None),
                Ok(_)
            );

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

    #[test]
    fn header_sync_gap_lookup() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let mut rng = generators::rng();
        let consensus_tip = rng.gen();
        let (_tip_tx, tip_rx) = watch::channel(consensus_tip);
        let mode = HeaderSyncMode::Tip(tip_rx);

        // Genesis
        let checkpoint = 0;
        let head = random_header(&mut rng, 0, None);
        let gap_fill = random_header(&mut rng, 1, Some(head.hash()));
        let gap_tip = random_header(&mut rng, 2, Some(gap_fill.hash()));

        // Empty database
        assert_matches!(
            provider.sync_gap(mode.clone(), checkpoint),
            Err(RethError::Provider(ProviderError::HeaderNotFound(block_number)))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        provider
            .tx_ref()
            .put::<tables::CanonicalHeaders>(head.number, head.hash())
            .expect("failed to write canonical");
        provider
            .tx_ref()
            .put::<tables::Headers>(head.number, head.clone().unseal())
            .expect("failed to write header");

        let gap = provider.sync_gap(mode.clone(), checkpoint).unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), consensus_tip.into());

        // Checkpoint and gap
        provider
            .tx_ref()
            .put::<tables::CanonicalHeaders>(gap_tip.number, gap_tip.hash())
            .expect("failed to write canonical");
        provider
            .tx_ref()
            .put::<tables::Headers>(gap_tip.number, gap_tip.clone().unseal())
            .expect("failed to write header");

        let gap = provider.sync_gap(mode.clone(), checkpoint).unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), gap_tip.parent_hash.into());

        // Checkpoint and gap closed
        provider
            .tx_ref()
            .put::<tables::CanonicalHeaders>(gap_fill.number, gap_fill.hash())
            .expect("failed to write canonical");
        provider
            .tx_ref()
            .put::<tables::Headers>(gap_fill.number, gap_fill.clone().unseal())
            .expect("failed to write header");

        assert_matches!(
            provider.sync_gap(mode, checkpoint),
            Err(RethError::Provider(ProviderError::InconsistentHeaderGap))
        );
    }
}
