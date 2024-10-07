use crate::{
    providers::{state::latest::LatestStateProvider, StaticFileProvider},
    to_range,
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory,
    EvmEnvProvider, HeaderProvider, HeaderSyncGap, HeaderSyncGapProvider, ProviderError,
    PruneCheckpointReader, RequestsProvider, StageCheckpointReader, StateProviderBox,
    StaticFileProviderFactory, TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, BlockHash, BlockNumber, TxHash, TxNumber, B256, U256};
use core::fmt;
use reth_chainspec::{ChainInfo, EthereumHardforks};
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::{database::Database, models::StoredBlockBodyIndices};
use reth_errors::{RethError, RethResult};
use reth_evm::ConfigureEvmEnv;
use reth_node_types::NodeTypesWithDB;
use reth_primitives::{
    Block, BlockWithSenders, Header, Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader,
    StaticFileSegment, TransactionMeta, TransactionSigned, TransactionSignedNoHash, Withdrawal,
    Withdrawals,
};
use reth_prune_types::{PruneCheckpoint, PruneModes, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::TryIntoHistoricalStateProvider;
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use std::{
    ops::{RangeBounds, RangeInclusive},
    path::Path,
    sync::Arc,
};
use tokio::sync::watch;
use tracing::trace;

mod provider;
pub use provider::{DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW};

use super::ProviderNodeTypes;

mod metrics;

/// A common provider that fetches data from a database or static file.
///
/// This provider implements most provider or provider factory traits.
pub struct ProviderFactory<N: NodeTypesWithDB> {
    /// Database
    db: N::DB,
    /// Chain spec
    chain_spec: Arc<N::ChainSpec>,
    /// Static File Provider
    static_file_provider: StaticFileProvider,
    /// Optional pruning configuration
    prune_modes: PruneModes,
}

impl<N> fmt::Debug for ProviderFactory<N>
where
    N: NodeTypesWithDB<DB: fmt::Debug, ChainSpec: fmt::Debug>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { db, chain_spec, static_file_provider, prune_modes } = self;
        f.debug_struct("ProviderFactory")
            .field("db", &db)
            .field("chain_spec", &chain_spec)
            .field("static_file_provider", &static_file_provider)
            .field("prune_modes", &prune_modes)
            .finish()
    }
}

impl<N: NodeTypesWithDB> ProviderFactory<N> {
    /// Create new database provider factory.
    pub fn new(
        db: N::DB,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider,
    ) -> Self {
        Self { db, chain_spec, static_file_provider, prune_modes: PruneModes::none() }
    }

    /// Enables metrics on the static file provider.
    pub fn with_static_files_metrics(mut self) -> Self {
        self.static_file_provider = self.static_file_provider.with_metrics();
        self
    }

    /// Sets the pruning configuration for an existing [`ProviderFactory`].
    pub fn with_prune_modes(mut self, prune_modes: PruneModes) -> Self {
        self.prune_modes = prune_modes;
        self
    }

    /// Returns reference to the underlying database.
    pub const fn db_ref(&self) -> &N::DB {
        &self.db
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Consumes Self and returns DB
    pub fn into_db(self) -> N::DB {
        self.db
    }
}

impl<N: NodeTypesWithDB<DB = Arc<DatabaseEnv>>> ProviderFactory<N> {
    /// Create new database provider by passing a path. [`ProviderFactory`] will own the database
    /// instance.
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        chain_spec: Arc<N::ChainSpec>,
        args: DatabaseArguments,
        static_file_provider: StaticFileProvider,
    ) -> RethResult<Self> {
        Ok(Self {
            db: Arc::new(init_db(path, args).map_err(RethError::msg)?),
            chain_spec,
            static_file_provider,
            prune_modes: PruneModes::none(),
        })
    }
}

impl<N: ProviderNodeTypes> ProviderFactory<N> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    ///
    /// This sets the [`PruneModes`] to [`None`], because they should only be relevant for writing
    /// data.
    #[track_caller]
    pub fn provider(&self) -> ProviderResult<DatabaseProviderRO<N::DB, N::ChainSpec>> {
        Ok(DatabaseProvider::new(
            self.db.tx()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
        ))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    #[track_caller]
    pub fn provider_rw(&self) -> ProviderResult<DatabaseProviderRW<N::DB, N::ChainSpec>> {
        Ok(DatabaseProviderRW(DatabaseProvider::new_rw(
            self.db.tx_mut()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
        )))
    }

    /// State provider for latest block
    #[track_caller]
    pub fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.db.tx()?, self.static_file_provider())))
    }

    /// Storage provider for state at that given block
    pub fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        let state_provider = self.provider()?.try_into_history_at_block(block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block number");
        Ok(state_provider)
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        let provider = self.provider()?;

        let block_number = provider
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        let state_provider = self.provider()?.try_into_history_at_block(block_number)?;
        trace!(target: "providers::db", ?block_number, %block_hash, "Returning historical state provider for block hash");
        Ok(state_provider)
    }
}

impl<N: ProviderNodeTypes> DatabaseProviderFactory for ProviderFactory<N> {
    type DB = N::DB;
    type Provider = DatabaseProvider<<N::DB as Database>::TX, N::ChainSpec>;
    type ProviderRW = DatabaseProvider<<N::DB as Database>::TXMut, N::ChainSpec>;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        self.provider()
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        self.provider_rw().map(|provider| provider.0)
    }
}

impl<N: NodeTypesWithDB> StaticFileProviderFactory for ProviderFactory<N> {
    /// Returns static file provider
    fn static_file_provider(&self) -> StaticFileProvider {
        self.static_file_provider.clone()
    }
}

impl<N: ProviderNodeTypes> HeaderSyncGapProvider for ProviderFactory<N> {
    fn sync_gap(
        &self,
        tip: watch::Receiver<B256>,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<HeaderSyncGap> {
        self.provider()?.sync_gap(tip, highest_uninterrupted_block)
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for ProviderFactory<N> {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            num,
            |static_file| static_file.header_by_number(num),
            || self.provider()?.header_by_number(num),
        )
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        if let Some(td) = self.chain_spec.final_paris_total_difficulty(number) {
            // if this block is higher than the final paris(merge) block, return the final paris
            // difficulty
            return Ok(Some(td))
        }

        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.header_td_by_number(number),
            || self.provider()?.header_td_by_number(number),
        )
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, _| static_file.headers_range(range),
            |range, _| self.provider()?.headers_range(range),
            |_| true,
        )
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.sealed_header(number),
            || self.provider()?.sealed_header(number),
        )
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.sealed_headers_while(range, |_| true)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, predicate| static_file.sealed_headers_while(range, predicate),
            |range, predicate| self.provider()?.sealed_headers_while(range, predicate),
            predicate,
        )
    }
}

impl<N: ProviderNodeTypes> BlockHashReader for ProviderFactory<N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.block_hash(number),
            || self.provider()?.block_hash(number),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            start..end,
            |static_file, range, _| static_file.canonical_hashes_range(range.start, range.end),
            |range, _| self.provider()?.canonical_hashes_range(range.start, range.end),
            |_| true,
        )
    }
}

impl<N: ProviderNodeTypes> BlockNumReader for ProviderFactory<N> {
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

impl<N: ProviderNodeTypes> BlockReader for ProviderFactory<N> {
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

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.provider()?.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        self.provider()?.block_range(range)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        self.provider()?.block_with_senders_range(range)
    }

    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        self.provider()?.sealed_block_with_senders_range(range)
    }
}

impl<N: ProviderNodeTypes> TransactionsProvider for ProviderFactory<N> {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id(id),
            || self.provider()?.transaction_by_id(id),
        )
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id_no_hash(id),
            || self.provider()?.transaction_by_id_no_hash(id),
        )
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

impl<N: ProviderNodeTypes> ReceiptProvider for ProviderFactory<N> {
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || self.provider()?.receipt(id),
        )
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
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.provider()?.receipts_by_tx_range(range),
            |_| true,
        )
    }
}

impl<N: ProviderNodeTypes> WithdrawalsProvider for ProviderFactory<N> {
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

impl<N: ProviderNodeTypes> RequestsProvider for ProviderFactory<N> {
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<reth_primitives::Requests>> {
        self.provider()?.requests_by_block(id, timestamp)
    }
}

impl<N: ProviderNodeTypes> StageCheckpointReader for ProviderFactory<N> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.provider()?.get_stage_checkpoint_progress(id)
    }
    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.provider()?.get_all_checkpoints()
    }
}

impl<N: ProviderNodeTypes> EvmEnvProvider for ProviderFactory<N> {
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        self.provider()?.fill_env_with_header(cfg, block_env, header, evm_config)
    }

    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        self.provider()?.fill_cfg_env_with_header(cfg, header, evm_config)
    }
}

impl<N: NodeTypesWithDB> ChainSpecProvider for ProviderFactory<N> {
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<N::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<N: ProviderNodeTypes> PruneCheckpointReader for ProviderFactory<N> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.provider()?.get_prune_checkpoint(segment)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.provider()?.get_prune_checkpoints()
    }
}

impl<N: NodeTypesWithDB> Clone for ProviderFactory<N> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            chain_spec: self.chain_spec.clone(),
            static_file_provider: self.static_file_provider.clone(),
            prune_modes: self.prune_modes.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::{StaticFileProvider, StaticFileWriter},
        test_utils::{blocks::TEST_BLOCK, create_test_provider_factory, MockNodeTypesWithDB},
        BlockHashReader, BlockNumReader, BlockWriter, HeaderSyncGapProvider, TransactionsProvider,
    };
    use alloy_primitives::{TxNumber, B256, U256};
    use assert_matches::assert_matches;
    use rand::Rng;
    use reth_chainspec::ChainSpecBuilder;
    use reth_db::{
        mdbx::DatabaseArguments,
        tables,
        test_utils::{create_test_static_files_dir, ERROR_TEMPDIR},
    };
    use reth_primitives::StaticFileSegment;
    use reth_prune_types::{PruneMode, PruneModes};
    use reth_storage_errors::provider::ProviderError;
    use reth_testing_utils::generators::{self, random_block, random_header, BlockParams};
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
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let factory = ProviderFactory::<MockNodeTypesWithDB<DatabaseEnv>>::new_with_database_path(
            tempfile::TempDir::new().expect(ERROR_TEMPDIR).into_path(),
            Arc::new(chain_spec),
            DatabaseArguments::new(Default::default()),
            StaticFileProvider::read_write(static_dir_path).unwrap(),
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

        let block = TEST_BLOCK.clone();
        {
            let provider = factory.provider_rw().unwrap();
            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap()),
                Ok(_)
            );
            assert_matches!(
                provider.transaction_sender(0), Ok(Some(sender))
                if sender == block.body.transactions[0].recover_signer().unwrap()
            );
            assert_matches!(provider.transaction_id(block.body.transactions[0].hash), Ok(Some(0)));
        }

        {
            let prune_modes = PruneModes {
                sender_recovery: Some(PruneMode::Full),
                transaction_lookup: Some(PruneMode::Full),
                ..PruneModes::none()
            };
            let provider = factory.with_prune_modes(prune_modes).provider_rw().unwrap();
            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap(),),
                Ok(_)
            );
            assert_matches!(provider.transaction_sender(0), Ok(None));
            assert_matches!(provider.transaction_id(block.body.transactions[0].hash), Ok(None));
        }
    }

    #[test]
    fn take_block_transaction_range_recover_senders() {
        let factory = create_test_provider_factory();

        let mut rng = generators::rng();
        let block =
            random_block(&mut rng, 0, BlockParams { tx_count: Some(3), ..Default::default() });

        let tx_ranges: Vec<RangeInclusive<TxNumber>> = vec![0..=0, 1..=1, 2..=2, 0..=1, 1..=2];
        for range in tx_ranges {
            let provider = factory.provider_rw().unwrap();

            assert_matches!(
                provider.insert_block(block.clone().try_seal_with_senders().unwrap()),
                Ok(_)
            );

            let senders = provider.take::<tables::TransactionSenders>(range.clone());
            assert_eq!(
                senders,
                Ok(range
                    .clone()
                    .map(|tx_number| (
                        tx_number,
                        block.body.transactions[tx_number as usize].recover_signer().unwrap()
                    ))
                    .collect())
            );

            let db_senders = provider.senders_by_tx_range(range);
            assert_eq!(db_senders, Ok(vec![]));

            let result = provider.take_block_transaction_range(0..=0);
            assert_eq!(
                result,
                Ok(vec![(
                    0,
                    block
                        .body
                        .transactions
                        .iter()
                        .cloned()
                        .map(|tx| tx.into_ecrecovered().unwrap())
                        .collect()
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

        // Genesis
        let checkpoint = 0;
        let head = random_header(&mut rng, 0, None);

        // Empty database
        assert_matches!(
            provider.sync_gap(tip_rx.clone(), checkpoint),
            Err(ProviderError::HeaderNotFound(block_number))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer =
            static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        static_file_writer.append_header(head.header(), U256::ZERO, &head.hash()).unwrap();
        static_file_writer.commit().unwrap();
        drop(static_file_writer);

        let gap = provider.sync_gap(tip_rx, checkpoint).unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), consensus_tip.into());
    }
}
