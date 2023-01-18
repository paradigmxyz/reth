//! Builder for a reth test instance.

use reth_cli_utils::init::init_genesis;
use reth_consensus::BeaconConsensus;
use reth_db::database::Database;
use reth_downloaders::{bodies, headers};
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::NetworkHandle;
use reth_primitives::{ChainSpec, H256};
use reth_stages::{
    metrics::HeaderMetrics,
    stages::{
        bodies::BodyStage, execution::ExecutionStage, headers::HeaderStage,
        sender_recovery::SenderRecoveryStage, total_difficulty::TotalDifficultyStage,
    },
    Pipeline,
};
use reth_tracing::tracing::{debug, info};
use std::sync::Arc;
use tokio::sync::watch::error::SendError;

use crate::stage_config::StageConfig;

/// Reth test instance
pub(crate) struct RethTestInstance<DB> {
    pub(crate) consensus: Arc<BeaconConsensus>,
    pub(crate) network: NetworkHandle,
    pub(crate) db: Arc<DB>,
    pub(crate) chain_spec: ChainSpec,
    pub(crate) tip: Option<H256>,
    pub(crate) config: StageConfig,
}

impl<DB> RethTestInstance<DB>
where
    DB: Database,
{
    /// Start the reth sync pipeline
    pub(crate) async fn start(&self) -> Result<(), RethTestInstanceError> {
        // make sure to init genesis if not done already
        let _genesis_hash = init_genesis(self.db.clone(), self.chain_spec.genesis().clone())?;

        // start pipeline
        let fetch_client = Arc::new(self.network.fetch_client().await.unwrap());
        let mut pipeline = Pipeline::default()
            .with_sync_state_updater(self.network.clone())
            .push(HeaderStage {
                downloader: headers::linear::LinearDownloadBuilder::default()
                    .batch_size(self.config.headers.downloader_batch_size)
                    .retries(self.config.headers.downloader_retries)
                    .build(self.consensus.clone(), fetch_client.clone()),
                consensus: self.consensus.clone(),
                client: fetch_client.clone(),
                network_handle: self.network.clone(),
                commit_threshold: self.config.headers.commit_threshold,
                metrics: HeaderMetrics::default(),
            })
            .push(TotalDifficultyStage {
                commit_threshold: self.config.total_difficulty.commit_threshold,
            })
            .push(BodyStage {
                downloader: Arc::new(
                    bodies::concurrent::ConcurrentDownloader::new(
                        fetch_client.clone(),
                        self.consensus.clone(),
                    )
                    .with_batch_size(self.config.bodies.downloader_batch_size)
                    .with_retries(self.config.bodies.downloader_retries)
                    .with_concurrency(self.config.bodies.downloader_concurrency),
                ),
                consensus: self.consensus.clone(),
                commit_threshold: self.config.bodies.commit_threshold,
            })
            .push(SenderRecoveryStage {
                batch_size: self.config.sender_recovery.batch_size,
                commit_threshold: self.config.sender_recovery.commit_threshold,
            })
            .push(ExecutionStage {
                chain_spec: self.chain_spec.clone(),
                commit_threshold: self.config.execution.commit_threshold,
            });

        if let Some(tip) = self.tip {
            debug!("Tip manually set: {}", tip);
            self.consensus.notify_fork_choice_state(ForkchoiceState {
                head_block_hash: tip,
                safe_block_hash: tip,
                finalized_block_hash: tip,
            })?;
        }

        // Run pipeline
        info!("Starting pipeline");
        pipeline.run(self.db.clone()).await?;
        Ok(())
    }
}

/// An error that can occur while starting reth.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RethTestInstanceError {
    /// Error while initializing the genesis block.
    #[error("Error while initializing the genesis block: {0}")]
    GenesisInitError(#[from] reth_db::Error),

    /// Error while notifying consensus listeners of a fork choice state update.
    #[error("Error while notifying consensus listeners of a fork choice state update: {0}")]
    ForkChoiceStateUpdateError(#[from] SendError<ForkchoiceState>),

    /// Error while running the reth pipeline.
    #[error("Error while running the reth pipeline: {0}")]
    PipelineError(#[from] reth_stages::PipelineError),
}

// TODO: config
/// Builder for a reth test instance.
pub(crate) struct RethBuilder<DB> {
    network: Option<NetworkHandle>,
    consensus: Option<Arc<BeaconConsensus>>,
    db: Option<Arc<DB>>,
    chain_spec: Option<ChainSpec>,
    tip: Option<H256>,
    stage_config: Option<StageConfig>,
}

impl<DB> RethBuilder<DB> {
    /// Creates a new builder.
    pub(crate) fn new() -> Self {
        Self {
            network: None,
            consensus: None,
            db: None,
            chain_spec: None,
            tip: None,
            stage_config: None,
        }
    }

    /// Sets the network handle.
    #[must_use]
    pub(crate) fn network(mut self, network: NetworkHandle) -> Self {
        self.network = Some(network);
        self
    }

    /// Sets the consensus handle.
    #[must_use]
    pub(crate) fn consensus(mut self, consensus: Arc<BeaconConsensus>) -> Self {
        self.consensus = Some(consensus);
        self
    }

    /// Sets the database handle.
    #[must_use]
    pub(crate) fn db(mut self, db: Arc<DB>) -> Self {
        self.db = Some(db);
        self
    }

    /// Sets the genesis block and chain config.
    #[must_use]
    pub(crate) fn chain_spec(mut self, chain_spec: ChainSpec) -> Self {
        self.chain_spec = Some(chain_spec);
        self
    }

    /// Sets the tip block hash for reverse download.
    #[must_use]
    pub(crate) fn tip(mut self, tip: H256) -> Self {
        self.tip = Some(tip);
        self
    }

    /// Sets the stage config.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn stage_config(mut self, stage_config: StageConfig) -> Self {
        self.stage_config = Some(stage_config);
        self
    }

    /// Builds the test instance.
    pub(crate) fn build(self) -> RethTestInstance<DB> {
        RethTestInstance {
            network: self.network.unwrap(),
            consensus: self.consensus.unwrap(),
            db: self.db.unwrap(),
            chain_spec: self.chain_spec.unwrap(),
            tip: self.tip,
            config: self.stage_config.unwrap_or_default(),
        }
    }
}
