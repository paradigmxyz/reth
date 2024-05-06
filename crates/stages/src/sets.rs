//! Built-in [`StageSet`]s.
//!
//! The easiest set to use is [`DefaultStages`], which provides all stages required to run an
//! instance of reth.
//!
//! It is also possible to run parts of reth standalone given the required data is present in
//! the environment, such as [`ExecutionStages`] or [`HashingStages`].
//!
//!
//! # Examples
//!
//! ```no_run
//! # use reth_stages::Pipeline;
//! # use reth_stages::sets::{OfflineStages};
//! # use reth_primitives::{PruneModes, MAINNET};
//! # use reth_evm_ethereum::EthEvmConfig;
//! # use reth_provider::StaticFileProviderFactory;
//! # use reth_provider::test_utils::create_test_provider_factory;
//! # use reth_static_file::StaticFileProducer;
//! # use reth_config::config::EtlConfig;
//! # use reth_evm::execute::BlockExecutorProvider;
//!
//! # fn create(exec: impl BlockExecutorProvider) {
//!
//! let provider_factory = create_test_provider_factory();
//! let static_file_producer = StaticFileProducer::new(
//!     provider_factory.clone(),
//!     provider_factory.static_file_provider(),
//!     PruneModes::default(),
//! );
//! // Build a pipeline with all offline stages.
//! let pipeline = Pipeline::builder()
//!     .add_stages(OfflineStages::new(exec, EtlConfig::default()))
//!     .build(provider_factory, static_file_producer);
//!
//! # }
//! ```
use crate::{
    stages::{
        AccountHashingStage, BodyStage, ExecutionStage, FinishStage, HeaderStage,
        IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage,
        StorageHashingStage, TransactionLookupStage,
    },
    StageSet, StageSetBuilder,
};
use reth_config::config::EtlConfig;
use reth_consensus::Consensus;
use reth_db::database::Database;
use reth_evm::execute::BlockExecutorProvider;
use reth_interfaces::p2p::{
    bodies::downloader::BodyDownloader, headers::downloader::HeaderDownloader,
};
use reth_provider::{HeaderSyncGapProvider, HeaderSyncMode};
use std::sync::Arc;

/// A set containing all stages to run a fully syncing instance of reth.
///
/// A combination of (in order)
///
/// - [`OnlineStages`]
/// - [`OfflineStages`]
/// - [`FinishStage`]
///
/// This expands to the following series of stages:
/// - [`HeaderStage`]
/// - [`BodyStage`]
/// - [`SenderRecoveryStage`]
/// - [`ExecutionStage`]
/// - [`MerkleStage`] (unwind)
/// - [`AccountHashingStage`]
/// - [`StorageHashingStage`]
/// - [`MerkleStage`] (execute)
/// - [`TransactionLookupStage`]
/// - [`IndexStorageHistoryStage`]
/// - [`IndexAccountHistoryStage`]
/// - [`FinishStage`]
#[derive(Debug)]
pub struct DefaultStages<Provider, H, B, EF> {
    /// Configuration for the online stages
    online: OnlineStages<Provider, H, B>,
    /// Executor factory needs for execution stage
    executor_factory: EF,
    /// ETL configuration
    etl_config: EtlConfig,
}

impl<Provider, H, B, E> DefaultStages<Provider, H, B, E> {
    /// Create a new set of default stages with default values.
    pub fn new(
        provider: Provider,
        header_mode: HeaderSyncMode,
        consensus: Arc<dyn Consensus>,
        header_downloader: H,
        body_downloader: B,
        executor_factory: E,
        etl_config: EtlConfig,
    ) -> Self
    where
        E: BlockExecutorProvider,
    {
        Self {
            online: OnlineStages::new(
                provider,
                header_mode,
                consensus,
                header_downloader,
                body_downloader,
                etl_config.clone(),
            ),
            executor_factory,
            etl_config,
        }
    }
}

impl<Provider, H, B, E> DefaultStages<Provider, H, B, E>
where
    E: BlockExecutorProvider,
{
    /// Appends the default offline stages and default finish stage to the given builder.
    pub fn add_offline_stages<DB: Database>(
        default_offline: StageSetBuilder<DB>,
        executor_factory: E,
        etl_config: EtlConfig,
    ) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_set(default_offline)
            .add_set(OfflineStages::new(executor_factory, etl_config))
            .add_stage(FinishStage)
    }
}

impl<Provider, H, B, E, DB> StageSet<DB> for DefaultStages<Provider, H, B, E>
where
    Provider: HeaderSyncGapProvider + 'static,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
    E: BlockExecutorProvider,
    DB: Database + 'static,
{
    fn builder(self) -> StageSetBuilder<DB> {
        Self::add_offline_stages(self.online.builder(), self.executor_factory, self.etl_config)
    }
}

/// A set containing all stages that require network access by default.
///
/// These stages *can* be run without network access if the specified downloaders are
/// themselves offline.
#[derive(Debug)]
pub struct OnlineStages<Provider, H, B> {
    /// Sync gap provider for the headers stage.
    provider: Provider,
    /// The sync mode for the headers stage.
    header_mode: HeaderSyncMode,
    /// The consensus engine used to validate incoming data.
    consensus: Arc<dyn Consensus>,
    /// The block header downloader
    header_downloader: H,
    /// The block body downloader
    body_downloader: B,
    /// ETL configuration
    etl_config: EtlConfig,
}

impl<Provider, H, B> OnlineStages<Provider, H, B> {
    /// Create a new set of online stages with default values.
    pub fn new(
        provider: Provider,
        header_mode: HeaderSyncMode,
        consensus: Arc<dyn Consensus>,
        header_downloader: H,
        body_downloader: B,
        etl_config: EtlConfig,
    ) -> Self {
        Self { provider, header_mode, consensus, header_downloader, body_downloader, etl_config }
    }
}

impl<Provider, H, B> OnlineStages<Provider, H, B>
where
    Provider: HeaderSyncGapProvider + 'static,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
{
    /// Create a new builder using the given headers stage.
    pub fn builder_with_headers<DB: Database>(
        headers: HeaderStage<Provider, H>,
        body_downloader: B,
    ) -> StageSetBuilder<DB> {
        StageSetBuilder::default().add_stage(headers).add_stage(BodyStage::new(body_downloader))
    }

    /// Create a new builder using the given bodies stage.
    pub fn builder_with_bodies<DB: Database>(
        bodies: BodyStage<B>,
        provider: Provider,
        mode: HeaderSyncMode,
        header_downloader: H,
        consensus: Arc<dyn Consensus>,
        etl_config: EtlConfig,
    ) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(HeaderStage::new(
                provider,
                header_downloader,
                mode,
                consensus.clone(),
                etl_config,
            ))
            .add_stage(bodies)
    }
}

impl<DB, Provider, H, B> StageSet<DB> for OnlineStages<Provider, H, B>
where
    DB: Database,
    Provider: HeaderSyncGapProvider + 'static,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
{
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(HeaderStage::new(
                self.provider,
                self.header_downloader,
                self.header_mode,
                self.consensus.clone(),
                self.etl_config.clone(),
            ))
            .add_stage(BodyStage::new(self.body_downloader))
    }
}

/// A set containing all stages that do not require network access.
///
/// A combination of (in order)
///
/// - [`ExecutionStages`]
/// - [`HashingStages`]
/// - [`HistoryIndexingStages`]
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct OfflineStages<EF> {
    /// Executor factory needs for execution stage
    pub executor_factory: EF,
    /// ETL configuration
    etl_config: EtlConfig,
}

impl<EF> OfflineStages<EF> {
    /// Create a new set of offline stages with default values.
    pub fn new(executor_factory: EF, etl_config: EtlConfig) -> Self {
        Self { executor_factory, etl_config }
    }
}

impl<E, DB> StageSet<DB> for OfflineStages<E>
where
    E: BlockExecutorProvider,
    DB: Database,
{
    fn builder(self) -> StageSetBuilder<DB> {
        ExecutionStages::new(self.executor_factory)
            .builder()
            .add_set(HashingStages { etl_config: self.etl_config.clone() })
            .add_set(HistoryIndexingStages { etl_config: self.etl_config })
    }
}

/// A set containing all stages that are required to execute pre-existing block data.
#[derive(Debug)]
#[non_exhaustive]
pub struct ExecutionStages<E> {
    /// Executor factory that will create executors.
    executor_factory: E,
}

impl<E> ExecutionStages<E> {
    /// Create a new set of execution stages with default values.
    pub fn new(executor_factory: E) -> Self {
        Self { executor_factory }
    }
}

impl<E, DB> StageSet<DB> for ExecutionStages<E>
where
    DB: Database,
    E: BlockExecutorProvider,
{
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(SenderRecoveryStage::default())
            .add_stage(ExecutionStage::new_with_executor(self.executor_factory))
    }
}

/// A set containing all stages that hash account state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HashingStages {
    /// ETL configuration
    etl_config: EtlConfig,
}

impl<DB: Database> StageSet<DB> for HashingStages {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(MerkleStage::default_unwind())
            .add_stage(AccountHashingStage::default().with_etl_config(self.etl_config.clone()))
            .add_stage(StorageHashingStage::default().with_etl_config(self.etl_config))
            .add_stage(MerkleStage::default_execution())
    }
}

/// A set containing all stages that do additional indexing for historical state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HistoryIndexingStages {
    /// ETL configuration
    etl_config: EtlConfig,
}

impl<DB: Database> StageSet<DB> for HistoryIndexingStages {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(TransactionLookupStage::default().with_etl_config(self.etl_config.clone()))
            .add_stage(IndexStorageHistoryStage::default().with_etl_config(self.etl_config.clone()))
            .add_stage(IndexAccountHistoryStage::default().with_etl_config(self.etl_config))
    }
}
