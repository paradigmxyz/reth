//! Main `stage` command
//!
//! Stage debugging tool
use crate::{
    config::Config,
    dirs::{ConfigPath, DbPath, PlatformPath},
    prometheus_exporter,
    utils::{
        chainspec::{chain_spec_value_parser, ChainSpecification},
        init::{init_db, init_genesis},
    },
    NetworkOpts,
};
use reth_consensus::BeaconConsensus;
use reth_downloaders::bodies::concurrent::ConcurrentDownloader;
use reth_executor::Config as ExecutorConfig;
use reth_stages::{
    metrics::HeaderMetrics,
    stages::{bodies::BodyStage, execution::ExecutionStage, sender_recovery::SenderRecoveryStage},
    ExecInput, Stage, StageId, Transaction, UnwindInput,
};

use clap::Parser;
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc};
use strum::{AsRefStr, EnumString, EnumVariantNames};
use tracing::*;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: PlatformPath<ConfigPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = chain_spec_value_parser
    )]
    chain: ChainSpecification,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[clap(long, value_name = "SOCKET")]
    metrics: Option<SocketAddr>,

    /// The name of the stage to run
    stage: StageEnum,

    /// The height to start at
    #[arg(long)]
    from: u64,

    /// The end of the stage
    #[arg(long, short)]
    to: u64,

    /// Normally, running the stage requires unwinding for stages that already
    /// have been run, in order to not rewrite to the same database slots.
    ///
    /// You can optionally skip the unwinding phase if you're syncing a block
    /// range that has not been synced before.
    #[arg(long, short)]
    skip_unwind: bool,

    #[clap(flatten)]
    network: NetworkOpts,
}

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, AsRefStr, EnumVariantNames, EnumString, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "kebab-case")]
enum StageEnum {
    Headers,
    Bodies,
    Senders,
    Execution,
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        fdlimit::raise_fd_limit();

        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", "Starting metrics endpoint at {}", listen_addr);
            prometheus_exporter::initialize(listen_addr)?;
            HeaderMetrics::describe();
        }

        let config: Config = confy::load_path(&self.config).unwrap_or_default();
        info!(target: "reth::cli", "reth {} starting stage {:?}", clap::crate_version!(), self.stage);

        let input = ExecInput {
            previous_stage: Some((StageId("No Previous Stage"), self.to)),
            stage_progress: Some(self.from),
        };

        let unwind = UnwindInput { stage_progress: self.to, unwind_to: self.from, bad_block: None };

        let db = Arc::new(init_db(&self.db)?);
        let mut tx = Transaction::new(db.as_ref())?;

        let num_blocks = self.to - self.from + 1;

        match self.stage {
            StageEnum::Bodies => {
                let chain_id = self.chain.consensus.chain_id;
                let consensus = Arc::new(BeaconConsensus::new(self.chain.consensus.clone()));
                let genesis_hash = init_genesis(db.clone(), self.chain.genesis.clone())?;

                let mut config = config;
                config.peers.connect_trusted_nodes_only = self.network.trusted_only;
                if !self.network.trusted_peers.is_empty() {
                    self.network.trusted_peers.iter().for_each(|peer| {
                        config.peers.trusted_nodes.insert(*peer);
                    });
                }

                let network = config
                    .network_config(
                        db.clone(),
                        chain_id,
                        genesis_hash,
                        self.network.disable_discovery,
                    )
                    .start_network()
                    .await?;
                let fetch_client = Arc::new(network.fetch_client().await?);

                let mut stage = BodyStage {
                    downloader: Arc::new(
                        ConcurrentDownloader::new(fetch_client.clone(), consensus.clone())
                            .with_batch_size(config.stages.bodies.downloader_batch_size)
                            .with_retries(config.stages.bodies.downloader_retries)
                            .with_concurrency(config.stages.bodies.downloader_concurrency),
                    ),
                    consensus: consensus.clone(),
                    commit_threshold: num_blocks,
                };

                if !self.skip_unwind {
                    stage.unwind(&mut tx, unwind).await?;
                }
                stage.execute(&mut tx, input).await?;
            }
            StageEnum::Senders => {
                let mut stage = SenderRecoveryStage {
                    batch_size: config.stages.sender_recovery.batch_size,
                    commit_threshold: num_blocks,
                };

                // Unwind first
                if !self.skip_unwind {
                    stage.unwind(&mut tx, unwind).await?;
                }
                stage.execute(&mut tx, input).await?;
            }
            StageEnum::Execution => {
                let mut stage = ExecutionStage {
                    config: ExecutorConfig::new_ethereum(),
                    commit_threshold: num_blocks,
                };
                if !self.skip_unwind {
                    stage.unwind(&mut tx, unwind).await?;
                }
                stage.execute(&mut tx, input).await?;
            }
            _ => {}
        }

        Ok(())
    }
}
