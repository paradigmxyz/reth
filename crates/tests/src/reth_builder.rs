//! Builder for a reth test instance.

use reth_cli_utils::{chainspec::Genesis, init::init_genesis};
use reth_consensus::BeaconConsensus;
use reth_db::database::Database;
use reth_downloaders::{bodies, headers};
use reth_executor::Config as ExecutorConfig;
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::NetworkHandle;
use reth_primitives::H256;
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

/// Reth test instance
pub struct RethTestInstance<DB> {
    pub consensus: Arc<BeaconConsensus>,
    pub network: NetworkHandle,
    pub db: Arc<DB>,
    pub genesis: Genesis,
    pub tip: Option<H256>,
}

// TODO: configs

impl<DB> RethTestInstance<DB>
where
    DB: Database,
{
    /// Start the reth sync pipeline
    pub async fn start(&self) -> Result<(), RethTestInstanceError> {
        // init genesis
        let genesis_hash = init_genesis(self.db.clone(), self.genesis.clone())?;

        // start pipeline
        let fetch_client = Arc::new(self.network.fetch_client().await.unwrap());
        let mut pipeline = Pipeline::default()
            .with_sync_state_updater(self.network.clone())
            .push(HeaderStage {
                downloader: headers::linear::LinearDownloadBuilder::default()
                    .batch_size(config.stages.headers.downloader_batch_size)
                    .retries(config.stages.headers.downloader_retries)
                    .build(self.consensus.clone(), fetch_client.clone()),
                consensus: self.consensus.clone(),
                client: fetch_client.clone(),
                network_handle: self.network.clone(),
                commit_threshold: config.stages.headers.commit_threshold,
                metrics: HeaderMetrics::default(),
            })
            .push(TotalDifficultyStage {
                commit_threshold: config.stages.total_difficulty.commit_threshold,
            })
            .push(BodyStage {
                downloader: Arc::new(
                    bodies::concurrent::ConcurrentDownloader::new(
                        fetch_client.clone(),
                        self.consensus.clone(),
                    )
                    .with_batch_size(config.stages.bodies.downloader_batch_size)
                    .with_retries(config.stages.bodies.downloader_retries)
                    .with_concurrency(config.stages.bodies.downloader_concurrency),
                ),
                consensus: self.consensus.clone(),
                commit_threshold: config.stages.bodies.commit_threshold,
            })
            .push(SenderRecoveryStage {
                batch_size: config.stages.sender_recovery.batch_size,
                commit_threshold: config.stages.sender_recovery.commit_threshold,
            })
            .push(ExecutionStage {
                config: ExecutorConfig::new_ethereum(),
                commit_threshold: config.stages.execution.commit_threshold,
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
pub enum RethTestInstanceError {
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
pub struct RethBuilder<DB> {
    network: Option<NetworkHandle>,
    consensus: Option<Arc<BeaconConsensus>>,
    db: Option<Arc<DB>>,
    genesis: Option<Genesis>,
    tip: Option<H256>,
}

impl<DB> Default for RethBuilder<DB> {
    fn default() -> Self {
        Self { network: None, consensus: None, db: None, genesis: None, tip: None }
    }
}

impl<DB> RethBuilder<DB> {
    /// Creates a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the network handle.
    pub fn network(mut self, network: NetworkHandle) -> Self {
        self.network = Some(network);
        self
    }

    /// Sets the consensus handle.
    pub fn consensus(mut self, consensus: Arc<BeaconConsensus>) -> Self {
        self.consensus = Some(consensus);
        self
    }

    /// Sets the database handle.
    pub fn db(mut self, db: Arc<DB>) -> Self {
        self.db = Some(db);
        self
    }

    /// Sets the genesis block and chain config.
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Sets the tip block hash for reverse download.
    pub fn tip(mut self, tip: H256) -> Self {
        self.tip = Some(tip);
        self
    }

    /// Builds the test instance.
    pub fn build(self) -> RethTestInstance<DB> {
        RethTestInstance {
            network: self.network.unwrap(),
            consensus: self.consensus.unwrap(),
            db: self.db.unwrap(),
            genesis: self.genesis.unwrap(),
            tip: self.tip,
        }
    }
}
