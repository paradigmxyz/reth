use crate::{
    providers::{
        state::latest::LatestStateProvider, NodeTypesForProvider, RocksDBProvider,
        StaticFileProvider, StaticFileProviderRWRefMut,
    },
    to_range,
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory,
    EitherWriterDestination, HashedPostStateProvider, HeaderProvider, HeaderSyncGapProvider,
    MetadataProvider, ProviderError, PruneCheckpointReader, RocksDBProviderFactory,
    StageCheckpointReader, StateProviderBox, StaticFileProviderFactory, StaticFileWriter,
    TransactionVariant, TransactionsProvider,
};
use alloy_consensus::transaction::TransactionMeta;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, BlockHash, BlockNumber, TxHash, TxNumber, B256};
use core::fmt;
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::{database::Database, models::StoredBlockBodyIndices};
use reth_errors::{RethError, RethResult};
use reth_node_types::{
    BlockTy, HeaderTy, NodeTypesWithDB, NodeTypesWithDBAdapter, ReceiptTy, TxTy,
};
use reth_primitives_traits::{RecoveredBlock, SealedHeader};
use reth_prune_types::{PruneCheckpoint, PruneModes, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, NodePrimitivesProvider, StorageSettings, StorageSettingsCache,
    TryIntoHistoricalStateProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::HashedPostState;
use revm_database::BundleState;
use std::{
    ops::{RangeBounds, RangeInclusive},
    path::Path,
    sync::Arc,
};

use tracing::trace;

mod provider;
pub use provider::{DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW, SaveBlocksMode};

use super::ProviderNodeTypes;
use reth_trie::KeccakKeyHasher;

mod builder;
pub use builder::{ProviderFactoryBuilder, ReadOnlyConfig};

mod metrics;

mod chain;
pub use chain::*;

/// A common provider that fetches data from a database or static file.
///
/// This provider implements most provider or provider factory traits.
pub struct ProviderFactory<N: NodeTypesWithDB> {
    /// Database instance
    db: N::DB,
    /// Chain spec
    chain_spec: Arc<N::ChainSpec>,
    /// Static File Provider
    static_file_provider: StaticFileProvider<N::Primitives>,
    /// Optional pruning configuration
    prune_modes: PruneModes,
    /// The node storage handler.
    storage: Arc<N::Storage>,
    /// Storage configuration settings for this node
    storage_settings: Arc<RwLock<StorageSettings>>,
    /// `RocksDB` provider
    rocksdb_provider: RocksDBProvider,
}

impl<N: NodeTypesForProvider> ProviderFactory<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>> {
    /// Instantiates the builder for this type
    pub fn builder() -> ProviderFactoryBuilder<N> {
        ProviderFactoryBuilder::default()
    }
}

impl<N: ProviderNodeTypes> ProviderFactory<N> {
    /// Create new database provider factory.
    pub fn new(
        db: N::DB,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        rocksdb_provider: RocksDBProvider,
    ) -> ProviderResult<Self> {
        // Load storage settings from database at init time. Creates a temporary provider
        // to read persisted settings, falling back to legacy defaults if none exist.
        //
        // Both factory and all providers it creates should share these cached settings.
        let legacy_settings = StorageSettings::legacy();
        let storage_settings = DatabaseProvider::<_, N>::new(
            db.tx()?,
            chain_spec.clone(),
            static_file_provider.clone(),
            Default::default(),
            Default::default(),
            Arc::new(RwLock::new(legacy_settings)),
            rocksdb_provider.clone(),
        )
        .storage_settings()?
        .unwrap_or(legacy_settings);

        Ok(Self {
            db,
            chain_spec,
            static_file_provider,
            prune_modes: PruneModes::default(),
            storage: Default::default(),
            storage_settings: Arc::new(RwLock::new(storage_settings)),
            rocksdb_provider,
        })
    }
}

impl<N: NodeTypesWithDB> ProviderFactory<N> {
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

impl<N: NodeTypesWithDB> StorageSettingsCache for ProviderFactory<N> {
    fn cached_storage_settings(&self) -> StorageSettings {
        *self.storage_settings.read()
    }

    fn set_storage_settings_cache(&self, settings: StorageSettings) {
        *self.storage_settings.write() = settings;
    }
}

impl<N: NodeTypesWithDB> RocksDBProviderFactory for ProviderFactory<N> {
    fn rocksdb_provider(&self) -> RocksDBProvider {
        self.rocksdb_provider.clone()
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    fn set_pending_rocksdb_batch(&self, _batch: rocksdb::WriteBatchWithTransaction<true>) {
        unimplemented!("ProviderFactory is a factory, not a provider - use DatabaseProvider::set_pending_rocksdb_batch instead")
    }
}

impl<N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>> ProviderFactory<N> {
    /// Create new database provider by passing a path. [`ProviderFactory`] will own the database
    /// instance.
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        chain_spec: Arc<N::ChainSpec>,
        args: DatabaseArguments,
        static_file_provider: StaticFileProvider<N::Primitives>,
        rocksdb_provider: RocksDBProvider,
    ) -> RethResult<Self> {
        Self::new(
            Arc::new(init_db(path, args).map_err(RethError::msg)?),
            chain_spec,
            static_file_provider,
            rocksdb_provider,
        )
        .map_err(RethError::Provider)
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
    pub fn provider(&self) -> ProviderResult<DatabaseProviderRO<N::DB, N>> {
        Ok(DatabaseProvider::new(
            self.db.tx()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
            self.storage.clone(),
            self.storage_settings.clone(),
            self.rocksdb_provider.clone(),
        ))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    #[track_caller]
    pub fn provider_rw(&self) -> ProviderResult<DatabaseProviderRW<N::DB, N>> {
        Ok(DatabaseProviderRW(DatabaseProvider::new_rw(
            self.db.tx_mut()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
            self.storage.clone(),
            self.storage_settings.clone(),
            self.rocksdb_provider.clone(),
        )))
    }

    /// State provider for latest block
    #[track_caller]
    pub fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.database_provider_ro()?)))
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

        let state_provider = provider.try_into_history_at_block(block_number)?;
        trace!(target: "providers::db", ?block_number, %block_hash, "Returning historical state provider for block hash");
        Ok(state_provider)
    }
}

impl<N: NodeTypesWithDB> NodePrimitivesProvider for ProviderFactory<N> {
    type Primitives = N::Primitives;
}

impl<N: ProviderNodeTypes> DatabaseProviderFactory for ProviderFactory<N> {
    type DB = N::DB;
    type Provider = DatabaseProvider<<N::DB as Database>::TX, N>;
    type ProviderRW = DatabaseProvider<<N::DB as Database>::TXMut, N>;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        self.provider()
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        self.provider_rw().map(|provider| provider.0)
    }
}

impl<N: NodeTypesWithDB> StaticFileProviderFactory for ProviderFactory<N> {
    /// Returns static file provider
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        self.static_file_provider.clone()
    }

    fn get_static_file_writer(
        &self,
        block: BlockNumber,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_, Self::Primitives>> {
        self.static_file_provider.get_writer(block, segment)
    }
}

impl<N: ProviderNodeTypes> HeaderSyncGapProvider for ProviderFactory<N> {
    type Header = HeaderTy<N>;
    fn local_tip_header(
        &self,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<SealedHeader<Self::Header>> {
        self.provider()?.local_tip_header(highest_uninterrupted_block)
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for ProviderFactory<N> {
    type Header = HeaderTy<N>;

    fn header(&self, block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.static_file_provider.header_by_number(num)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.static_file_provider.headers_range(range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_headers_range(range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_headers_while(range, predicate)
    }
}

impl<N: ProviderNodeTypes> BlockHashReader for ProviderFactory<N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.canonical_hashes_range(start, end)
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
        self.static_file_provider.last_block_number()
    }

    fn earliest_block_number(&self) -> ProviderResult<BlockNumber> {
        // earliest history height tracks the lowest block number that has __not__ been expired, in
        // other words, the first/earliest available block.
        Ok(self.static_file_provider.earliest_history_height())
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.block_number(hash)
    }
}

impl<N: ProviderNodeTypes> BlockReader for ProviderFactory<N> {
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        self.provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        self.provider()?.block(id)
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.provider()?.pending_block()
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
        self.provider()?.pending_block_and_receipts()
    }

    fn recovered_block(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.provider()?.recovered_block(id, transaction_kind)
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.provider()?.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        self.provider()?.block_range(range)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.provider()?.block_with_senders_range(range)
    }

    fn recovered_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.provider()?.recovered_block_range(range)
    }

    fn block_by_transaction_id(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.block_by_transaction_id(id)
    }
}

impl<N: ProviderNodeTypes> TransactionsProvider for ProviderFactory<N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.transaction_by_id(id)
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.transaction_by_id_unhashed(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        self.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        self.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        self.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        self.provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.static_file_provider.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.static_file_provider.senders_by_tx_range(range)
        } else {
            self.provider()?.senders_by_tx_range(range)
        }
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.static_file_provider.transaction_sender(id)
        } else {
            self.provider()?.transaction_sender(id)
        }
    }
}

impl<N: ProviderNodeTypes> ReceiptProvider for ProviderFactory<N> {
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || self.provider()?.receipt(id),
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        self.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.provider()?.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.provider()?.receipts_by_tx_range(range),
            |_| true,
        )
    }

    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        self.provider()?.receipts_by_block_range(block_range)
    }
}

impl<N: ProviderNodeTypes> BlockBodyIndicesProvider for ProviderFactory<N> {
    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices(number)
    }

    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices_range(range)
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

impl<N: ProviderNodeTypes> HashedPostStateProvider for ProviderFactory<N> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<N: ProviderNodeTypes> MetadataProvider for ProviderFactory<N> {
    fn get_metadata(&self, key: &str) -> ProviderResult<Option<Vec<u8>>> {
        self.provider()?.get_metadata(key)
    }
}

impl<N> fmt::Debug for ProviderFactory<N>
where
    N: NodeTypesWithDB<DB: fmt::Debug, ChainSpec: fmt::Debug, Storage: fmt::Debug>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            db,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
        } = self;
        f.debug_struct("ProviderFactory")
            .field("db", &db)
            .field("chain_spec", &chain_spec)
            .field("static_file_provider", &static_file_provider)
            .field("prune_modes", &prune_modes)
            .field("storage", &storage)
            .field("storage_settings", &*storage_settings.read())
            .field("rocksdb_provider", &rocksdb_provider)
            .finish()
    }
}

impl<N: NodeTypesWithDB> Clone for ProviderFactory<N> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            chain_spec: self.chain_spec.clone(),
            static_file_provider: self.static_file_provider.clone(),
            prune_modes: self.prune_modes.clone(),
            storage: self.storage.clone(),
            storage_settings: self.storage_settings.clone(),
            rocksdb_provider: self.rocksdb_provider.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::{StaticFileProvider, StaticFileWriter},
        test_utils::{blocks::TEST_BLOCK, create_test_provider_factory, MockNodeTypesWithDB},
        BlockHashReader, BlockNumReader, BlockWriter, DBProvider, HeaderSyncGapProvider,
        TransactionsProvider,
    };
    use alloy_primitives::{TxNumber, B256};
    use assert_matches::assert_matches;
    use reth_chainspec::ChainSpecBuilder;
    use reth_db::{
        mdbx::DatabaseArguments,
        test_utils::{create_test_rocksdb_dir, create_test_static_files_dir, ERROR_TEMPDIR},
    };
    use reth_db_api::tables;
    use reth_primitives_traits::SignerRecoverable;
    use reth_prune_types::{PruneMode, PruneModes};
    use reth_storage_errors::provider::ProviderError;
    use reth_testing_utils::generators::{self, random_block, random_header, BlockParams};
    use std::{ops::RangeInclusive, sync::Arc};

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
        let (_, rocksdb_path) = create_test_rocksdb_dir();
        let factory = ProviderFactory::<MockNodeTypesWithDB<DatabaseEnv>>::new_with_database_path(
            tempfile::TempDir::new().expect(ERROR_TEMPDIR).keep(),
            Arc::new(chain_spec),
            DatabaseArguments::new(Default::default()),
            StaticFileProvider::read_write(static_dir_path).unwrap(),
            RocksDBProvider::builder(&rocksdb_path).with_default_tables().build().unwrap(),
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
        let block = TEST_BLOCK.clone();

        {
            let factory = create_test_provider_factory();
            let provider = factory.provider_rw().unwrap();
            assert_matches!(provider.insert_block(&block.clone().try_recover().unwrap()), Ok(_));
            assert_matches!(
                provider.transaction_sender(0), Ok(Some(sender))
                if sender == block.body().transactions[0].recover_signer().unwrap()
            );
            assert_matches!(
                provider.transaction_id(*block.body().transactions[0].tx_hash()),
                Ok(Some(0))
            );
        }

        {
            let prune_modes = PruneModes {
                sender_recovery: Some(PruneMode::Full),
                transaction_lookup: Some(PruneMode::Full),
                ..PruneModes::default()
            };
            let factory = create_test_provider_factory();
            let provider = factory.with_prune_modes(prune_modes).provider_rw().unwrap();
            assert_matches!(provider.insert_block(&block.clone().try_recover().unwrap()), Ok(_));
            assert_matches!(provider.transaction_sender(0), Ok(None));
            assert_matches!(
                provider.transaction_id(*block.body().transactions[0].tx_hash()),
                Ok(None)
            );
        }
    }

    #[test]
    fn take_block_transaction_range_recover_senders() {
        let mut rng = generators::rng();
        let block =
            random_block(&mut rng, 0, BlockParams { tx_count: Some(3), ..Default::default() });

        let tx_ranges: Vec<RangeInclusive<TxNumber>> = vec![0..=0, 1..=1, 2..=2, 0..=1, 1..=2];
        for range in tx_ranges {
            let factory = create_test_provider_factory();
            let provider = factory.provider_rw().unwrap();

            assert_matches!(provider.insert_block(&block.clone().try_recover().unwrap()), Ok(_));

            let senders = provider.take::<tables::TransactionSenders>(range.clone()).unwrap();
            assert_eq!(
                senders,
                range
                    .clone()
                    .map(|tx_number| (
                        tx_number,
                        block.body().transactions[tx_number as usize].recover_signer().unwrap()
                    ))
                    .collect::<Vec<_>>()
            );

            let db_senders = provider.senders_by_tx_range(range);
            assert!(matches!(db_senders, Ok(ref v) if v.is_empty()));
        }
    }

    #[test]
    fn header_sync_gap_lookup() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let mut rng = generators::rng();

        // Genesis
        let checkpoint = 0;
        let head = random_header(&mut rng, 0, None);

        // Empty database
        assert_matches!(
            provider.local_tip_header(checkpoint),
            Err(ProviderError::HeaderNotFound(block_number))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer =
            static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        static_file_writer.append_header(head.header(), &head.hash()).unwrap();
        static_file_writer.commit().unwrap();
        drop(static_file_writer);

        let local_head = provider.local_tip_header(checkpoint).unwrap();

        assert_eq!(local_head, head);
    }
}
