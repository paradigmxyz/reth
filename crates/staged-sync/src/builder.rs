use crate::config::{BodiesConfig, ExecutionConfig, SenderRecoveryConfig};

use super::config::HeadersConfig;
use reth_db::database::Database;
use reth_downloaders::{
    bodies::concurrent::ConcurrentDownloader,
    headers::linear::{LinearDownloadBuilder, LinearDownloader},
};
use reth_interfaces::{
    consensus::Consensus,
    sync::{NoopSyncStateUpdate, SyncStateUpdater},
};
use reth_network::{FetchClient, NetworkHandle};
use reth_primitives::{ChainSpec, ChainSpecBuilder};
use reth_stages::{
    metrics::HeaderMetrics,
    stages::{
        bodies::BodyStage, execution::ExecutionStage, headers::HeaderStage,
        sender_recovery::SenderRecoveryStage,
    },
    Pipeline,
};

use std::sync::Arc;

use eyre::Result;

/// Rough API usage:
///
/// 1. To launch the pipeline
/// RethBuilder::new(..).online(..).with_headers(..).with_bodies(..).with_execution(..).build()
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
pub struct RethBuilder<DB> {
    db: Arc<DB>,

    senders_recovery: Option<SenderRecoveryConfig>,

    chain_spec: Option<ChainSpec>,
    execution: Option<ExecutionConfig>,
}

impl<DB: Database> RethBuilder<DB> {
    pub fn new(db: DB) -> Self {
        Self { db: Arc::new(db), senders_recovery: None, execution: None, chain_spec: None }
    }

    pub fn with_senders_recovery(mut self, config: SenderRecoveryConfig) -> Self {
        self.senders_recovery = Some(config);
        self
    }

    pub fn senders_recovery(&self) -> Option<SenderRecoveryStage> {
        self.senders_recovery.as_ref().map(|config| SenderRecoveryStage {
            batch_size: config.batch_size,
            commit_threshold: config.commit_threshold,
        })
    }

    pub fn with_execution(mut self, config: ExecutionConfig) -> Self {
        self.execution = Some(config);
        self
    }

    pub fn with_chain_spec(mut self, chain_spec: ChainSpec) -> Self {
        self.chain_spec = Some(chain_spec);
        self
    }

    pub fn execution(&self) -> Option<ExecutionStage> {
        let chain_spec = self.chain_spec.unwrap_or(ChainSpecBuilder::mainnet().build());
        self.execution
            .as_ref()
            .map(|config| ExecutionStage { chain_spec, commit_threshold: config.commit_threshold })
    }

    pub fn configure_pipeline<U: SyncStateUpdater>(
        &self,
        mut pipeline: Pipeline<DB, U>,
    ) -> Pipeline<DB, U> {
        if let Some(stage) = self.senders_recovery() {
            pipeline = pipeline.push(stage);
        }

        if let Some(stage) = self.execution() {
            pipeline = pipeline.push(stage);
        }

        pipeline
    }

    pub fn online<C, N>(self, consensus: C, network: N) -> OnlineRethBuilder<C, N, DB> {
        OnlineRethBuilder::new(self, consensus, network)
    }
}

#[must_use = "need to call `build` on this struct"]
pub struct OnlineRethBuilder<C, N, DB> {
    builder: RethBuilder<DB>,
    consensus: Arc<C>,
    network: N,

    headers: Option<HeadersConfig>,
    bodies: Option<BodiesConfig>,
}

impl<C, N, DB: Database> OnlineRethBuilder<C, N, DB> {
    pub fn new(builder: RethBuilder<DB>, consensus: C, network: N) -> OnlineRethBuilder<C, N, DB> {
        Self { builder, consensus: Arc::new(consensus), network, headers: None, bodies: None }
    }
}

impl<C, DB> OnlineRethBuilder<C, NetworkHandle, DB>
where
    C: Consensus + 'static,
    DB: Database,
{
    pub fn with_headers_downloader(mut self, config: HeadersConfig) -> Self {
        self.headers = Some(config);
        self
    }

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

    pub async fn build(&self) -> Result<()> {
        // TODO: Add bodies.
        if self.headers.is_none() && self.bodies.is_none() {
            return Err(eyre::eyre!("No online stages configured, cnosider removing the `online` call from your builder or add a stage"));
        }

        // Instantiate the networked pipeline
        let mut pipeline =
            Pipeline::<DB, _>::default().with_sync_state_updater(self.network.clone());

        if let Some(stage) = self.headers_stage().await? {
            pipeline = pipeline.push(stage);
        }

        if let Some(stage) = self.bodies_stage().await? {
            pipeline = pipeline.push(stage);
        }

        pipeline = self.builder.configure_pipeline(pipeline);

        pipeline.run(self.builder.db.clone()).await?;

        Ok(())
    }
}
