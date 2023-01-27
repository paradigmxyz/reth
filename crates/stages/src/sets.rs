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
//! # use reth_db::mdbx::{Env, WriteMap};
//! # use reth_interfaces::sync::NoopSyncStateUpdate;
//! # use reth_stages::Pipeline;
//! # use reth_stages::sets::{OfflineStages};
//! // Build a pipeline with all offline stages.
//! # let pipeline: Pipeline<Env<WriteMap>, NoopSyncStateUpdate> =
//! Pipeline::builder().add_stages(OfflineStages).build();
//! ```
//!
//! ```ignore
//! # use reth_stages::Pipeline;
//! # use reth_stages::{StageSet, sets::OfflineStages};
//! // Build a pipeline with all offline stages and a custom stage at the end.
//! Pipeline::builder()
//!     .add_stages(
//!         OfflineStages.build().add_stage(MyCustomStage)
//!     )
//!     .build();
//! ```
use crate::{
    stages::{
        AccountHashingStage, BodyStage, ExecutionStage, HeaderStage, MerkleStage,
        SenderRecoveryStage, StorageHashingStage, TotalDifficultyStage,
    },
    StageSet, StageSetBuilder,
};
use reth_db::database::Database;
use reth_interfaces::{
    consensus::Consensus,
    p2p::{bodies::downloader::BodyDownloader, headers::downloader::HeaderDownloader},
};
use reth_primitives::ChainSpec;
use std::sync::Arc;

/// A set containing all stages to run a fully syncing instance of reth.
///
/// A combination of (in order)
///
/// - [`OnlineStages`]
/// - [`OfflineStages`]
#[derive(Debug)]
pub struct DefaultStages<H, B> {
    /// Configuration for the online stages
    online: OnlineStages<H, B>,
}

impl<H, B> DefaultStages<H, B> {
    /// Create a new set of default stages with default values.
    pub fn new(consensus: Arc<dyn Consensus>, header_downloader: H, body_downloader: B) -> Self {
        Self { online: OnlineStages::new(consensus, header_downloader, body_downloader) }
    }
}

impl<DB, H, B> StageSet<DB> for DefaultStages<H, B>
where
    DB: Database,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
{
    fn build(self) -> StageSetBuilder<DB> {
        self.online.build().add_set(OfflineStages)
    }
}

/// A set containing all stages that require network access by default.
///
/// These stages *can* be run without network access if the specified downloaders are
/// themselves offline.
#[derive(Debug)]
pub struct OnlineStages<H, B> {
    /// The consensus engine used to validate incoming data.
    consensus: Arc<dyn Consensus>,
    /// The block header downloader
    header_downloader: H,
    /// The block body downloader
    body_downloader: B,
}

impl<H, B> OnlineStages<H, B> {
    /// Create a new set of online stages with default values.
    pub fn new(consensus: Arc<dyn Consensus>, header_downloader: H, body_downloader: B) -> Self {
        Self { consensus, header_downloader, body_downloader }
    }
}

impl<DB, H, B> StageSet<DB> for OnlineStages<H, B>
where
    DB: Database,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
{
    fn build(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(HeaderStage::new(self.header_downloader, self.consensus.clone()))
            .add_stage(TotalDifficultyStage::default())
            .add_stage(BodyStage { downloader: self.body_downloader, consensus: self.consensus })
    }
}

/// A set containing all stages that do not require network access.
///
/// A combination of (in order)
///
/// - [`ExecutionStages`]
/// - [`HashingStages`]
#[derive(Debug)]
pub struct OfflineStages;

impl<DB: Database> StageSet<DB> for OfflineStages {
    fn build(self) -> StageSetBuilder<DB> {
        ExecutionStages::default().build().add_set(HashingStages)
    }
}

/// A set containing all stages that are required to execute pre-existing block data.
#[derive(Debug)]
pub struct ExecutionStages {
    /// The chain specification to use for execution.
    chain_spec: ChainSpec,
}

impl Default for ExecutionStages {
    fn default() -> Self {
        Self { chain_spec: reth_primitives::MAINNET.clone() }
    }
}

impl ExecutionStages {
    /// Create a new set of execution stages with default values.
    pub fn new(chain_spec: ChainSpec) -> Self {
        Self { chain_spec }
    }
}

impl<DB: Database> StageSet<DB> for ExecutionStages {
    fn build(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(SenderRecoveryStage::default())
            .add_stage(ExecutionStage { chain_spec: self.chain_spec, ..Default::default() })
    }
}

/// A set containing all stages that hash account state.
#[derive(Debug)]
pub struct HashingStages;

impl<DB: Database> StageSet<DB> for HashingStages {
    fn build(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default()
            .add_stage(MerkleStage::Unwind)
            .add_stage(AccountHashingStage::default())
            .add_stage(StorageHashingStage::default())
            .add_stage(MerkleStage::Execution)
    }
}
