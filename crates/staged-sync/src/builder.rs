use crate::config::{BodiesConfig, ExecutionConfig, SenderRecoveryConfig, TotalDifficultyConfig};

use super::config::HeadersConfig;
use reth_consensus::BeaconConsensus;
use reth_db::database::Database;
use reth_downloaders::{
    bodies::concurrent::ConcurrentDownloader,
    headers::linear::{LinearDownloadBuilder, LinearDownloader},
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    sync::{NoopSyncStateUpdate, SyncStateUpdater},
};
use reth_network::{FetchClient, NetworkHandle};
use reth_primitives::{ChainSpec, ChainSpecBuilder};
use reth_stages::{
    metrics::HeaderMetrics,
    stages::{
        bodies::BodyStage, execution::ExecutionStage, hashing_account::AccountHashingStage,
        hashing_storage::StorageHashingStage, headers::HeaderStage, merkle::MerkleStage,
        sender_recovery::SenderRecoveryStage, total_difficulty::TotalDifficultyStage,
    },
    Pipeline, PipelineEvent,
};

use std::sync::Arc;
use tokio::sync::mpsc::Sender;

use eyre::Result;

/// Rough API usage:
///
/// 1. To launch the pipeline
/// RethBuilder::new(..).with_execution(..).online(..).with_headers(..).with_bodies(..).build()
/// RethBuilder::new(..).with_execution(..).build()
///
/// OR
///
/// 2. To make it easy to run an individual stage:
///
/// let builder = RethBuilder::new(..).online(..).with_headers(..);
/// let header = builder.headers_stage().await?.expect("should be configured");
/// let tx = Transaction::new(builder.db());
/// let input = ExecInput {
///     previous_stage: Some((StageId("No Previous Stage"), self.to)),
///     stage_progress: Some(self.from),
/// };
/// stage.execute(&mut tx, input).await?;
#[must_use = "need to call `build` on this struct"]
pub struct RethBuilder {
    senders_recovery: Option<SenderRecoveryConfig>,

    chain_spec: Option<ChainSpec>,
    execution: Option<ExecutionConfig>,

    merklize: bool,

    channel: Option<Sender<PipelineEvent>>,
}

impl RethBuilder {
    pub fn new() -> Self {
        Self {
            senders_recovery: None,
            execution: None,
            chain_spec: None,
            merklize: false,
            channel: None,
        }
    }

    /// Converts the RethBuilder to an online one to be integrated with the Headers/Bodies stages
    pub fn online<C, N>(self, consensus: C, network: N) -> OnlineRethBuilder<C, N> {
        OnlineRethBuilder::new(self, consensus, network)
    }

    /// Configures the [`SenderRecoveryStage`]
    pub fn with_senders_recovery(mut self, config: SenderRecoveryConfig) -> Self {
        self.senders_recovery = Some(config);
        self
    }

    /// Retrieves the [`SenderRecoveryStage`] if it was configured
    pub fn senders_recovery(&self) -> Option<SenderRecoveryStage> {
        self.senders_recovery.as_ref().map(|config| SenderRecoveryStage {
            batch_size: config.batch_size,
            commit_threshold: config.commit_threshold,
        })
    }

    /// Configures the [`ExecutionStage`] parameters. The Default chainspec is mainnet,
    /// use [`Self::with_chain_spec`] to configure for other chains.
    pub fn with_execution(mut self, config: ExecutionConfig) -> Self {
        self.execution = Some(config);
        self
    }

    /// Configures [`ExecutionStage`]'s [`ChainSpec`]
    pub fn with_chain_spec(mut self, chain_spec: ChainSpec) -> Self {
        self.chain_spec = Some(chain_spec);
        self
    }

    /// Set a channel the pipeline will transmit events over (see [PipelineEvent]).
    pub fn with_channel(mut self, sender: Sender<PipelineEvent>) -> Self {
        self.channel = Some(sender);
        self
    }

    /// Configures the Merklization-related stages: MerkleStage, AccountHashingStage,
    /// StorageHashingStage
    pub fn with_merklization(mut self) -> Self {
        self.merklize = true;
        self
    }

    /// Retrieves the [`ExecutionStage`] if it was configured
    pub fn execution(&self) -> Option<ExecutionStage> {
        let chain_spec =
            self.chain_spec.clone().unwrap_or_else(|| ChainSpecBuilder::mainnet().build());
        self.execution
            .as_ref()
            .map(|config| ExecutionStage { chain_spec, commit_threshold: config.commit_threshold })
    }

    /// Given a [`Pipeline`] it proceeds to push the internally configured stages to it.
    pub fn configure_pipeline<DB: Database, U: SyncStateUpdater>(
        &self,
        mut pipeline: Pipeline<DB, U>,
    ) -> Pipeline<DB, U> {
        if let Some(stage) = self.senders_recovery() {
            pipeline = pipeline.push(stage);
        }

        if let Some(stage) = self.execution() {
            pipeline = pipeline.push(stage);
        }

        // Push all the merklization stages together
        // TODO: Add config
        if self.merklize {
            // This Merkle stage is used only on unwind
            pipeline = pipeline
                .push(MerkleStage { is_execute: false })
                .push(AccountHashingStage { clean_threshold: 500_000, commit_threshold: 100_000 })
                .push(StorageHashingStage { clean_threshold: 500_000, commit_threshold: 100_000 })
                // This merkle stage is used only for execute
                .push(MerkleStage { is_execute: true });
        }

        if let Some(ref channel) = self.channel {
            pipeline = pipeline.with_channel(channel.clone());
        }

        pipeline
    }

    /// Builds and runs the pipeline
    pub async fn run<DB: Database>(&self, db: Arc<DB>) -> Result<()> {
        // Instantiate the networked pipeline
        let pipeline = Pipeline::<DB, NoopSyncStateUpdate>::default();
        let mut pipeline = self.configure_pipeline(pipeline);
        pipeline.run(db).await?;
        Ok(())
    }
}

#[must_use = "need to call `build` on this struct"]
pub struct OnlineRethBuilder<C, N> {
    builder: RethBuilder,
    consensus: Arc<C>,
    network: N,

    headers: Option<HeadersConfig>,
    total_difficulty: Option<TotalDifficultyConfig>,
    bodies: Option<BodiesConfig>,
}

impl<C, N> OnlineRethBuilder<C, N> {
    pub fn new(builder: RethBuilder, consensus: C, network: N) -> OnlineRethBuilder<C, N> {
        Self {
            builder,
            consensus: Arc::new(consensus),
            network,
            headers: None,
            total_difficulty: None,
            bodies: None,
        }
    }
}

impl<U> OnlineRethBuilder<Arc<BeaconConsensus>, U> {
    /// When downloading from the Beacon Chain, headers are downloaded in reverse.
    /// Normally, the Consensus Layer (CL) would feed us with a tip, however we can also
    /// manually set it via this function to sync without a CL.
    pub fn with_tip(self, tip: reth_primitives::H256) -> Result<Self> {
        self.consensus.notify_fork_choice_state(ForkchoiceState {
            head_block_hash: tip,
            safe_block_hash: tip,
            finalized_block_hash: tip,
        })?;

        Ok(self)
    }
}

impl<C> OnlineRethBuilder<C, NetworkHandle>
where
    C: Consensus + 'static,
{
    /// Configures the [`HeaderStage`]
    pub fn with_headers_downloader(mut self, config: HeadersConfig) -> Self {
        self.headers = Some(config);
        self
    }

    /// Configures the [`TotalDifficultyStage`]
    pub fn with_total_difficulty(mut self, config: TotalDifficultyConfig) -> Self {
        self.total_difficulty = Some(config);
        self
    }

    /// Configures the [`BodyStage`]
    pub fn with_bodies_downloader(mut self, config: BodiesConfig) -> Self {
        self.bodies = Some(config);
        self
    }

    /// Returns the currently configured `HeaderStage` if a `HeadersConfig` has been provided.
    pub async fn headers_stage(
        &self,
    ) -> Result<Option<HeaderStage<LinearDownloader<C, FetchClient>, C, FetchClient, NetworkHandle>>>
    {
        let fetch_client = Arc::new(self.network.fetch_client().await?);
        Ok::<_, eyre::Error>(self.headers.as_ref().map(|config| {
            let downloader = LinearDownloadBuilder::default()
                .request_limit(config.downloader_batch_size)
                .stream_batch_size(config.commit_threshold as usize)
                // NOTE: the head and target will be set from inside the stage before the
                // downloader is called
                .build(
                    self.consensus.clone(),
                    fetch_client.clone(),
                    Default::default(),
                    Default::default(),
                );

            HeaderStage {
                downloader,
                consensus: self.consensus.clone(),
                client: fetch_client.clone(),
                network_handle: self.network.clone(),
                metrics: HeaderMetrics::default(),
            }
        }))
    }

    /// Retrieves the [`TotalDifficultyStage`] if it was configured
    pub fn total_difficulty_stage(&self) -> Option<TotalDifficultyStage> {
        self.total_difficulty
            .as_ref()
            .map(|config| TotalDifficultyStage { commit_threshold: config.commit_threshold })
    }

    /// Returns the currently configured `BodyStage` if a `BodiesConfig` has been provided.
    pub async fn bodies_stage(
        &self,
    ) -> Result<Option<BodyStage<ConcurrentDownloader<FetchClient, C>, C>>> {
        let fetch_client = Arc::new(self.network.fetch_client().await?);
        Ok::<_, eyre::Error>(self.bodies.as_ref().map(|config| {
            let downloader =
                ConcurrentDownloader::new(fetch_client.clone(), self.consensus.clone())
                    .with_batch_size(config.downloader_batch_size)
                    .with_retries(config.downloader_retries)
                    .with_concurrency(config.downloader_concurrency);

            BodyStage {
                downloader: Arc::new(downloader),
                consensus: self.consensus.clone(),
                commit_threshold: config.commit_threshold,
            }
        }))
    }

    /// Given a [`Pipeline`] it proceeds to push the internally configured stages to it.
    pub async fn configure_pipeline<DB: Database, U: SyncStateUpdater>(
        &self,
        mut pipeline: Pipeline<DB, U>,
    ) -> Result<Pipeline<DB, U>> {
        if let Some(stage) = self.headers_stage().await? {
            pipeline = pipeline.push(stage);
        }

        if let Some(stage) = self.total_difficulty_stage() {
            pipeline = pipeline.push(stage);
        }

        if let Some(stage) = self.bodies_stage().await? {
            pipeline = pipeline.push(stage);
        }

        if pipeline.stages.is_empty() {
            return Err(eyre::eyre!("No online stages configured, cnosider removing the `online` call from your builder or add a stage"));
        }

        Ok(pipeline)
    }

    /// Builds and runs the pipeline
    pub async fn run<DB: Database>(&self, db: Arc<DB>) -> Result<()> {
        // Instantiate the networked pipeline
        let mut pipeline =
            Pipeline::<DB, _>::default().with_sync_state_updater(self.network.clone());

        // Set the online stages
        pipeline = self.configure_pipeline(pipeline).await?;

        // Set the offline stages
        pipeline = self.builder.configure_pipeline(pipeline);

        pipeline.run(db).await?;

        Ok(())
    }
}
