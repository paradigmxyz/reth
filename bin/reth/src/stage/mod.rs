//! Main `stage` command
//!
//! Stage debugging tool
use crate::{
    args::NetworkArgs,
    dirs::{ConfigPath, DbPath, PlatformPath},
    prometheus_exporter, StageEnum,
};
use clap::Parser;
use reth_beacon_consensus::BeaconConsensus;
use reth_downloaders::bodies::bodies::BodiesDownloaderBuilder;
use reth_primitives::ChainSpec;
use reth_provider::{ShareableDatabase, Transaction};
use reth_staged_sync::{
    utils::{chainspec::chain_spec_value_parser, init::init_db},
    Config,
};
use reth_stages::{
    stages::{BodyStage, ExecutionStage, SenderRecoveryStage},
    ExecInput, Stage, StageId, UnwindInput,
};
use std::{net::SocketAddr, sync::Arc};
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
    chain: Arc<ChainSpec>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[clap(long, value_name = "SOCKET")]
    metrics: Option<SocketAddr>,

    /// The name of the stage to run
    #[arg(value_enum)]
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
    network: NetworkArgs,
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        fdlimit::raise_fd_limit();

        let config: Config = confy::load_path(&self.config).unwrap_or_default();
        info!(target: "reth::cli", "reth {} starting stage {:?}", clap::crate_version!(), self.stage);

        let input = ExecInput {
            previous_stage: Some((StageId("No Previous Stage"), self.to)),
            stage_progress: Some(self.from),
        };

        let unwind = UnwindInput { stage_progress: self.to, unwind_to: self.from, bad_block: None };

        let db = Arc::new(init_db(&self.db)?);
        let mut tx = Transaction::new(db.as_ref())?;

        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", "Starting metrics endpoint at {}", listen_addr);
            prometheus_exporter::initialize_with_db_metrics(listen_addr, Arc::clone(&db)).await?;
        }

        let num_blocks = self.to - self.from + 1;

        match self.stage {
            StageEnum::Bodies => {
                let consensus = Arc::new(BeaconConsensus::new(self.chain.clone()));

                let mut config = config;
                config.peers.connect_trusted_nodes_only = self.network.trusted_only;
                if !self.network.trusted_peers.is_empty() {
                    self.network.trusted_peers.iter().for_each(|peer| {
                        config.peers.trusted_nodes.insert(*peer);
                    });
                }

                let network = self
                    .network
                    .network_config(&config, self.chain.clone())
                    .build(Arc::new(ShareableDatabase::new(db.clone(), self.chain.clone())))
                    .start_network()
                    .await?;
                let fetch_client = Arc::new(network.fetch_client().await?);

                let mut stage = BodyStage {
                    downloader: BodiesDownloaderBuilder::default()
                        .with_stream_batch_size(num_blocks as usize)
                        .with_request_limit(config.stages.bodies.downloader_request_limit)
                        .with_max_buffered_responses(
                            config.stages.bodies.downloader_max_buffered_responses,
                        )
                        .with_concurrent_requests_range(
                            config.stages.bodies.downloader_min_concurrent_requests..=
                                config.stages.bodies.downloader_max_concurrent_requests,
                        )
                        .build(fetch_client.clone(), consensus.clone(), db.clone()),
                    consensus: consensus.clone(),
                };

                if !self.skip_unwind {
                    stage.unwind(&mut tx, unwind).await?;
                }
                stage.execute(&mut tx, input).await?;
            }
            StageEnum::Senders => {
                let mut stage = SenderRecoveryStage { commit_threshold: num_blocks };

                // Unwind first
                if !self.skip_unwind {
                    stage.unwind(&mut tx, unwind).await?;
                }
                stage.execute(&mut tx, input).await?;
            }
            StageEnum::Execution => {
                let factory = reth_revm::Factory::new(self.chain.clone());
                let mut stage = ExecutionStage::new(factory, num_blocks);
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
