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
//! # use reth_revm::Factory;
//! # use reth_primitives::MAINNET;
//! use reth_db::mdbx::test_utils::create_test_rw_db;
//!
//! # let factory = Factory::new(MAINNET.clone());
//! # let db = create_test_rw_db();
//! // Build a pipeline with all offline stages.
//! # let pipeline =
//! Pipeline::builder().add_stages(OfflineStages::new(factory)).build(db);
//! ```
//!
//! ```ignore
//! # use reth_stages::Pipeline;
//! # use reth_stages::{StageSet, sets::OfflineStages};
//! # use reth_revm::Factory;
//! # use reth_primitives::MAINNET;
//! // Build a pipeline with all offline stages and a custom stage at the end.
//! # let factory = Factory::new(MAINNET.clone());
//! Pipeline::builder()
//!     .add_stages(
//!         OfflineStages::new(factory).builder().add_stage(MyCustomStage)
//!     )
//!     .build();
//! ```
use crate::{
    stages::{
        AccountHashingStage, BodyStage, ExecutionStage, FinishStage, HeaderStage, HeaderSyncMode,
        IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage,
        StorageHashingStage, TotalDifficultyStage, TransactionLookupStage,
    },
    StageSet, StageSetBuilder,
};
use reth_db::database::Database;
use reth_interfaces::{
    consensus::Consensus,
    p2p::{bodies::downloader::BodyDownloader, headers::downloader::HeaderDownloader},
};
use reth_provider::ExecutorFactory;
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
/// - [`TotalDifficultyStage`]
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
pub struct DefaultStages<H, B, EF> {
    /// Configuration for the online stages
    online: OnlineStages<H, B>,
    /// Executor factory needs for execution stage
    executor_factory: EF,
}

impl<H, B, EF> DefaultStages<H, B, EF> {
    /// Create a new set of default stages with default values.
    pub fn new(
        header_mode: HeaderSyncMode,
        consensus: Arc<dyn Consensus>,
        header_downloader: H,
        body_downloader: B,
        executor_factory: EF,
    ) -> Self
    where
        EF: ExecutorFactory,
    {
        Self {
            online: OnlineStages::new(header_mode, consensus, header_downloader, body_downloader),
            executor_factory,
        }
    }
}

impl<H, B, EF> DefaultStages<H, B, EF>
where
    EF: ExecutorFactory,
{
    /// Appends the default offline stages and default finish stage to the given builder.
    pub fn add_offline_stages<DB: Database>(
        default_offline: StageSetBuilder<DB>,
        executor_factory: EF,
    ) -> StageSetBuilder<DB> {
        default_offline.add_set(OfflineStages::new(executor_factory)).add_stage(FinishStage)
    }
}

impl<DB, H, B, EF> StageSet<DB> for DefaultStages<H, B, EF>
where
    DB: Database,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
    EF: ExecutorFactory,
{
    fn builder(self) -> StageSetBuilder<DB> {
        Self::add_offline_stages(self.online.builder(), self.executor_factory)
    }
}

/// A set containing all stages that require network access by default.
///
/// These stages *can* be run without network access if the specified downloaders are
/// themselves offline.
#[derive(Debug)]
pub struct OnlineStages<H, B> {
    /// The sync mode for the headers stage.
    header_mode: HeaderSyncMode,
    /// The consensus engine used to validate incoming data.
    consensus: Arc<dyn Consensus>,
    /// The block header downloader
    header_downloader: H,
    /// The block body downloader
    body_downloader: B,
}

impl<H, B> OnlineStages<H, B> {
    /// Create a new set of online stages with default values.
    pub fn new(
        header_mode: HeaderSyncMode,
        consensus: Arc<dyn Consensus>,
        header_downloader: H,
        body_downloader: B,
    ) -> Self {
        Self { header_mode, consensus, header_downloader, body_downloader }
    }
}

impl<H, B> OnlineStages<H, B>
where
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
{
    /// Create a new builder using the given headers stage.
    pub fn builder_with_headers<DB: Database>(
        headers: HeaderStage<H>,
        body_downloader: B,
        consensus: Arc<dyn Consensus>,
    ) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(headers)
            .add_stage(TotalDifficultyStage::new(consensus.clone()))
            .add_stage(BodyStage { downloader: body_downloader, consensus })
    }

    /// Create a new builder using the given bodies stage.
    pub fn builder_with_bodies<DB: Database>(
        bodies: BodyStage<B>,
        mode: HeaderSyncMode,
        header_downloader: H,
        consensus: Arc<dyn Consensus>,
    ) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(HeaderStage::new(header_downloader, mode))
            .add_stage(TotalDifficultyStage::new(consensus.clone()))
            .add_stage(bodies)
    }
}

impl<DB, H, B> StageSet<DB> for OnlineStages<H, B>
where
    DB: Database,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
{
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(HeaderStage::new(self.header_downloader, self.header_mode))
            .add_stage(TotalDifficultyStage::new(self.consensus.clone()))
            .add_stage(BodyStage { downloader: self.body_downloader, consensus: self.consensus })
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
pub struct OfflineStages<EF: ExecutorFactory> {
    /// Executor factory needs for execution stage
    pub executor_factory: EF,
}

impl<EF: ExecutorFactory> OfflineStages<EF> {
    /// Create a new set of offline stages with default values.
    pub fn new(executor_factory: EF) -> Self {
        Self { executor_factory }
    }
}

impl<EF: ExecutorFactory, DB: Database> StageSet<DB> for OfflineStages<EF> {
    fn builder(self) -> StageSetBuilder<DB> {
        ExecutionStages::new(self.executor_factory)
            .builder()
            .add_set(HashingStages)
            .add_set(HistoryIndexingStages)
    }
}

/// A set containing all stages that are required to execute pre-existing block data.
#[derive(Debug)]
#[non_exhaustive]
pub struct ExecutionStages<EF: ExecutorFactory> {
    /// Executor factory that will create executors.
    executor_factory: EF,
}

impl<EF: ExecutorFactory + 'static> ExecutionStages<EF> {
    /// Create a new set of execution stages with default values.
    pub fn new(executor_factory: EF) -> Self {
        Self { executor_factory }
    }
}

impl<EF: ExecutorFactory, DB: Database> StageSet<DB> for ExecutionStages<EF> {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(SenderRecoveryStage::default())
            .add_stage(ExecutionStage::new_with_factory(self.executor_factory))
    }
}

/// A set containing all stages that hash account state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HashingStages;

impl<DB: Database> StageSet<DB> for HashingStages {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(MerkleStage::default_unwind())
            .add_stage(AccountHashingStage::default())
            .add_stage(StorageHashingStage::default())
            .add_stage(MerkleStage::default_execution())
    }
}

/// A set containing all stages that do additional indexing for historical state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct HistoryIndexingStages;

impl<DB: Database> StageSet<DB> for HistoryIndexingStages {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(TransactionLookupStage::default())
            .add_stage(IndexStorageHistoryStage::default())
            .add_stage(IndexAccountHistoryStage::default())
    }
}
