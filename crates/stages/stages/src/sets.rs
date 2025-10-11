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
//! # use reth_chainspec::MAINNET;
//! # use reth_prune_types::PruneModes;
//! # use reth_evm_ethereum::EthEvmConfig;
//! # use reth_evm::ConfigureEvm;
//! # use reth_provider::StaticFileProviderFactory;
//! # use reth_provider::test_utils::{create_test_provider_factory, MockNodeTypesWithDB};
//! # use reth_static_file::StaticFileProducer;
//! # use reth_config::config::StageConfig;
//! # use reth_ethereum_primitives::EthPrimitives;
//! # use std::sync::Arc;
//! # use reth_consensus::{FullConsensus, ConsensusError};
//!
//! # fn create(exec: impl ConfigureEvm<Primitives = EthPrimitives> + 'static, consensus: impl FullConsensus<EthPrimitives, Error = ConsensusError> + 'static) {
//!
//! let provider_factory = create_test_provider_factory();
//! let static_file_producer =
//!     StaticFileProducer::new(provider_factory.clone(), PruneModes::default());
//! // Build a pipeline with all offline stages.
//! let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
//!     .add_stages(OfflineStages::new(exec, Arc::new(consensus), StageConfig::default(), PruneModes::default()))
//!     .build(provider_factory, static_file_producer);
//!
//! # }
//! ```
use crate::{
    stages::{
        AccountHashingStage, BodyStage, EraImportSource, EraStage, ExecutionStage, FinishStage,
        HeaderStage, IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage,
        PruneSenderRecoveryStage, PruneStage, SenderRecoveryStage, StorageHashingStage,
        TransactionLookupStage,
    },
    StageSet, StageSetBuilder,
};
use alloy_primitives::B256;
use reth_config::config::StageConfig;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_evm::ConfigureEvm;
use reth_network_p2p::{bodies::downloader::BodyDownloader, headers::downloader::HeaderDownloader, snap::client::SnapClient};
use reth_primitives_traits::{Block, NodePrimitives};
use reth_provider::HeaderSyncGapProvider;
use reth_prune_types::PruneModes;
use reth_stages_api::Stage;
use std::{ops::Not, sync::Arc};
use tokio::sync::watch;


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
/// - [`PruneSenderRecoveryStage`] (execute)
/// - [`MerkleStage`] (unwind)
/// - [`AccountHashingStage`]
/// - [`StorageHashingStage`]
/// - [`MerkleStage`] (execute)
/// - [`TransactionLookupStage`]
/// - [`IndexStorageHistoryStage`]
/// - [`IndexAccountHistoryStage`]
/// - [`PruneStage`] (execute)
/// - [`FinishStage`]
#[derive(Debug)]
pub struct DefaultStages<Provider, H, B, E>
where
    H: HeaderDownloader,
    B: BodyDownloader,
    E: ConfigureEvm,
{
    /// Configuration for the online stages
    online: OnlineStages<Provider, H, B>,
    /// Executor factory needs for execution stage
    evm_config: E,
    /// Consensus instance
    consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
    /// Configuration for each stage in the pipeline
    stages_config: StageConfig,
    /// Prune configuration for every segment that can be pruned
    prune_modes: PruneModes,
}

impl<Provider, H, B, E> DefaultStages<Provider, H, B, E>
where
    H: HeaderDownloader,
    B: BodyDownloader,
    E: ConfigureEvm<Primitives: NodePrimitives<BlockHeader = H::Header, Block = B::Block>>,
{
    /// Create a new set of default stages with default values.
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        provider: Provider,
        tip: watch::Receiver<B256>,
        consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
        header_downloader: H,
        body_downloader: B,
        evm_config: E,
        stages_config: StageConfig,
        prune_modes: PruneModes,
        era_import_source: Option<EraImportSource>,
    ) -> Self {
        Self {
            online: OnlineStages::new(
                provider,
                tip,
                header_downloader,
                body_downloader,
                stages_config.clone(),
                era_import_source,
            ),
            evm_config,
            consensus,
            stages_config,
            prune_modes,
        }
    }
}

impl<P, H, B, E> DefaultStages<P, H, B, E>
where
    E: ConfigureEvm,
    H: HeaderDownloader,
    B: BodyDownloader,
{
    /// Appends the default offline stages and default finish stage to the given builder.
    pub fn add_offline_stages<Provider>(
        default_offline: StageSetBuilder<Provider>,
        evm_config: E,
        consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
        stages_config: StageConfig,
        prune_modes: PruneModes,
    ) -> StageSetBuilder<Provider>
    where
        OfflineStages<E>: StageSet<Provider>,
    {
        StageSetBuilder::default()
            .add_set(default_offline)
            .add_set(OfflineStages::new(evm_config, consensus, stages_config, prune_modes))
            .add_stage(FinishStage)
    }
}

impl<P, H, B, E, Provider> StageSet<Provider> for DefaultStages<P, H, B, E>
where
    P: HeaderSyncGapProvider + 'static,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
    E: ConfigureEvm,
    OnlineStages<P, H, B>: StageSet<Provider>,
    OfflineStages<E>: StageSet<Provider>,
{
    fn builder(self) -> StageSetBuilder<Provider> {
        Self::add_offline_stages(
            self.online.builder(),
            self.evm_config,
            self.consensus,
            self.stages_config.clone(),
            self.prune_modes,
        )
    }
}

/// A set containing all stages that require network access by default.
///
/// These stages *can* be run without network access if the specified downloaders are
/// themselves offline.
#[derive(Debug)]
pub struct OnlineStages<Provider, H, B>
where
    H: HeaderDownloader,
    B: BodyDownloader,
{
    /// Sync gap provider for the headers stage.
    provider: Provider,
    /// The tip for the headers stage.
    tip: watch::Receiver<B256>,

    /// The block header downloader
    header_downloader: H,
    /// The block body downloader
    body_downloader: B,
    /// Configuration for each stage in the pipeline
    stages_config: StageConfig,
    /// Optional source of ERA1 files. The `EraStage` does nothing unless this is specified.
    era_import_source: Option<EraImportSource>,
}

impl<Provider, H, B> OnlineStages<Provider, H, B>
where
    H: HeaderDownloader,
    B: BodyDownloader,
{
    /// Create a new set of online stages with default values.
    pub const fn new(
        provider: Provider,
        tip: watch::Receiver<B256>,
        header_downloader: H,
        body_downloader: B,
        stages_config: StageConfig,
        era_import_source: Option<EraImportSource>,
    ) -> Self {
        Self { provider, tip, header_downloader, body_downloader, stages_config, era_import_source }
    }
}

impl<P, H, B> OnlineStages<P, H, B>
where
    P: HeaderSyncGapProvider + 'static,
    H: HeaderDownloader<Header = <B::Block as Block>::Header> + 'static,
    B: BodyDownloader + 'static,
{
    /// Create a new builder using the given headers stage.
    pub fn builder_with_headers<Provider>(
        headers: HeaderStage<P, H>,
        body_downloader: B,
    ) -> StageSetBuilder<Provider>
    where
        HeaderStage<P, H>: Stage<Provider>,
        BodyStage<B>: Stage<Provider>,
    {
        StageSetBuilder::default().add_stage(headers).add_stage(BodyStage::new(body_downloader))
    }

    /// Create a new builder using the given bodies stage.
    pub fn builder_with_bodies<Provider>(
        bodies: BodyStage<B>,
        provider: P,
        tip: watch::Receiver<B256>,
        header_downloader: H,
        stages_config: StageConfig,
    ) -> StageSetBuilder<Provider>
    where
        BodyStage<B>: Stage<Provider>,
        HeaderStage<P, H>: Stage<Provider>,
    {
        StageSetBuilder::default()
            .add_stage(HeaderStage::new(provider, header_downloader, tip, stages_config.etl))
            .add_stage(bodies)
    }
}

impl<Provider, P, H, B> StageSet<Provider> for OnlineStages<P, H, B>
where
    P: HeaderSyncGapProvider + 'static,
    H: HeaderDownloader<Header = <B::Block as Block>::Header> + 'static,
    B: BodyDownloader + 'static,
    HeaderStage<P, H>: Stage<Provider>,
    BodyStage<B>: Stage<Provider>,
    EraStage<<B::Block as Block>::Header, <B::Block as Block>::Body, EraImportSource>:
        Stage<Provider>,
{
    fn builder(self) -> StageSetBuilder<Provider> {
        StageSetBuilder::default()
            .add_stage(EraStage::new(self.era_import_source, self.stages_config.etl.clone()))
            .add_stage(HeaderStage::new(
                self.provider,
                self.header_downloader,
                self.tip,
                self.stages_config.etl.clone(),
            ))
            .add_stage(BodyStage::new(self.body_downloader))
    }
}

/// A set containing all stages that do not require network access.
///
/// A combination of (in order)
///
/// - [`ExecutionStages`]
/// - [`PruneSenderRecoveryStage`]
/// - [`HashingStages`]
/// - [`HistoryIndexingStages`]
/// - [`PruneStage`]
#[derive(Debug)]
#[non_exhaustive]
pub struct OfflineStages<E: ConfigureEvm> {
    /// Executor factory needs for execution stage
    evm_config: E,
    /// Consensus instance for validating blocks.
    consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
    /// Configuration for each stage in the pipeline
    stages_config: StageConfig,
    /// Prune configuration for every segment that can be pruned
    prune_modes: PruneModes,
}

impl<E: ConfigureEvm> OfflineStages<E> {
    /// Create a new set of offline stages with default values.
    pub const fn new(
        evm_config: E,
        consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
        stages_config: StageConfig,
        prune_modes: PruneModes,
    ) -> Self {
        Self { evm_config, consensus, stages_config, prune_modes }
    }
}

impl<E, Provider> StageSet<Provider> for OfflineStages<E>
where
    E: ConfigureEvm,
    ExecutionStages<E>: StageSet<Provider>,
    PruneSenderRecoveryStage: Stage<Provider>,
    HashingStages: StageSet<Provider>,
    HistoryIndexingStages: StageSet<Provider>,
    PruneStage: Stage<Provider>,
{
    fn builder(self) -> StageSetBuilder<Provider> {
        ExecutionStages::new(self.evm_config, self.consensus, self.stages_config.clone())
            .builder()
            // If sender recovery prune mode is set, add the prune sender recovery stage.
            .add_stage_opt(self.prune_modes.sender_recovery.map(|prune_mode| {
                PruneSenderRecoveryStage::new(prune_mode, self.stages_config.prune.commit_threshold)
            }))
            .add_set(HashingStages { stages_config: self.stages_config.clone() })
            .add_set(HistoryIndexingStages {
                stages_config: self.stages_config.clone(),
                prune_modes: self.prune_modes.clone(),
            })
            // If any prune modes are set, add the prune stage.
            .add_stage_opt(self.prune_modes.is_empty().not().then(|| {
                // Prune stage should be added after all hashing stages, because otherwise it will
                // delete
                PruneStage::new(self.prune_modes.clone(), self.stages_config.prune.commit_threshold)
            }))
    }
}

/// A set containing all stages that are required to execute pre-existing block data.
#[derive(Debug)]
#[non_exhaustive]
pub struct ExecutionStages<E: ConfigureEvm, S = ()> {
    /// Executor factory that will create executors.
    evm_config: E,
    /// Consensus instance for validating blocks.
    consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
    /// Configuration for each stage in the pipeline
    stages_config: StageConfig,
    /// Optional snap client for snap sync (when enabled)
    /// `SnapSyncStage` is integrated into the pipeline when snap sync is enabled
    snap_client: Option<Arc<S>>,
}

impl<E: ConfigureEvm> ExecutionStages<E> {
    /// Create a new set of execution stages with default values.
    pub const fn new(
        executor_provider: E,
        consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
        stages_config: StageConfig,
    ) -> Self {
        Self { 
            evm_config: executor_provider, 
            consensus, 
            stages_config,
            snap_client: None,
        }
    }
}

impl<E: ConfigureEvm, S: SnapClient + Send + Sync + 'static> ExecutionStages<E, S> {
    /// Create a new set of execution stages with snap client.
    pub fn with_snap_client(
        executor_provider: E,
        consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
        stages_config: StageConfig,
        snap_client: Option<Arc<S>>,
    ) -> Self {
        Self { 
            evm_config: executor_provider, 
            consensus, 
            stages_config,
            snap_client,
        }
    }
}

impl<E, S, Provider> StageSet<Provider> for ExecutionStages<E, S>
where
    E: ConfigureEvm + 'static,
    S: reth_network_p2p::snap::client::SnapClient + Send + Sync + 'static,
    Provider: reth_provider::DBProvider + reth_provider::StatsReader + reth_provider::HeaderProvider,
    SenderRecoveryStage: Stage<Provider>,
    ExecutionStage<E>: Stage<Provider>,
    crate::stages::SnapSyncStage<S>: Stage<Provider>,
{
    fn builder(self) -> StageSetBuilder<Provider> {
        let mut builder = StageSetBuilder::default();
        
        // Check if snap sync is enabled
        if self.stages_config.snap_sync.enabled {
            // Use snap sync stage when enabled - REPLACES SenderRecoveryStage, ExecutionStage, PruneSenderRecoveryStage
            if let Some(snap_client) = self.snap_client {
                builder = builder.add_stage(crate::stages::SnapSyncStage::new(
                    self.stages_config.snap_sync,
                    snap_client,
                ));
            } else {
                // Fall back to traditional stages if snap client not provided
                builder = builder
                    .add_stage(SenderRecoveryStage::new(self.stages_config.sender_recovery))
                    .add_stage(ExecutionStage::from_config(
                        self.evm_config,
                        self.consensus,
                        self.stages_config.execution,
                        self.stages_config.execution_external_clean_threshold(),
                    ));
            }
        } else {
            // Use traditional stages when snap sync is disabled
            builder = builder
                .add_stage(SenderRecoveryStage::new(self.stages_config.sender_recovery))
                .add_stage(ExecutionStage::from_config(
                    self.evm_config,
                    self.consensus,
                    self.stages_config.execution,
                    self.stages_config.execution_external_clean_threshold(),
                ));
        }
            
        builder
    }
}

/// A set containing all stages that hash account state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HashingStages {
    /// Configuration for each stage in the pipeline
    stages_config: StageConfig,
}

impl<Provider> StageSet<Provider> for HashingStages
where
    MerkleStage: Stage<Provider>,
    AccountHashingStage: Stage<Provider>,
    StorageHashingStage: Stage<Provider>,
{
    fn builder(self) -> StageSetBuilder<Provider> {
        StageSetBuilder::default()
            .add_stage(MerkleStage::default_unwind())
            .add_stage(AccountHashingStage::new(
                self.stages_config.account_hashing,
                self.stages_config.etl.clone(),
            ))
            .add_stage(StorageHashingStage::new(
                self.stages_config.storage_hashing,
                self.stages_config.etl.clone(),
            ))
            .add_stage(MerkleStage::new_execution(
                self.stages_config.merkle.rebuild_threshold,
                self.stages_config.merkle.incremental_threshold,
            ))
    }
}

/// A set containing all stages that do additional indexing for historical state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HistoryIndexingStages {
    /// Configuration for each stage in the pipeline
    stages_config: StageConfig,
    /// Prune configuration for every segment that can be pruned
    prune_modes: PruneModes,
}

impl<Provider> StageSet<Provider> for HistoryIndexingStages
where
    TransactionLookupStage: Stage<Provider>,
    IndexStorageHistoryStage: Stage<Provider>,
    IndexAccountHistoryStage: Stage<Provider>,
{
    fn builder(self) -> StageSetBuilder<Provider> {
        StageSetBuilder::default()
            .add_stage(TransactionLookupStage::new(
                self.stages_config.transaction_lookup,
                self.stages_config.etl.clone(),
                self.prune_modes.transaction_lookup,
            ))
            .add_stage(IndexStorageHistoryStage::new(
                self.stages_config.index_storage_history,
                self.stages_config.etl.clone(),
                self.prune_modes.storage_history,
            ))
            .add_stage(IndexAccountHistoryStage::new(
                self.stages_config.index_account_history,
                self.stages_config.etl.clone(),
                self.prune_modes.account_history,
            ))
    }
}
