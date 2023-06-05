//! Main `stage` command
//!
//! Stage debugging tool
use crate::{
    args::{get_secret_key, utils::chain_spec_value_parser, NetworkArgs, StageEnum},
    dirs::{DataDirPath, MaybePlatformPath},
    prometheus_exporter,
    version::SHORT_VERSION,
};
use clap::Parser;
use reth_beacon_consensus::BeaconConsensus;
use reth_config::Config;
use reth_downloaders::bodies::bodies::BodiesDownloaderBuilder;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    ChainSpec,
};
use reth_provider::{providers::get_stage_checkpoint, ShareableDatabase, Transaction};
use reth_staged_sync::utils::init::init_db;
use reth_stages::{
    stages::{
        AccountHashingStage, BodyStage, ExecutionStage, ExecutionStageThresholds,
        IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage,
        StorageHashingStage, TransactionLookupStage,
    },
    ExecInput, ExecOutput, Stage, UnwindInput,
};
use std::{any::Any, net::SocketAddr, ops::Deref, path::PathBuf, sync::Arc};
use tracing::*;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

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

    /// Batch size for stage execution and unwind
    #[arg(long)]
    batch_size: Option<u64>,

    /// Normally, running the stage requires unwinding for stages that already
    /// have been run, in order to not rewrite to the same database slots.
    ///
    /// You can optionally skip the unwinding phase if you're syncing a block
    /// range that has not been synced before.
    #[arg(long, short)]
    skip_unwind: bool,

    #[clap(flatten)]
    network: NetworkArgs,

    /// Commits the changes in the database. WARNING: potentially destructive.
    ///
    /// Useful when you want to run diagnostics on the database.
    // TODO: We should consider allowing to run hooks at the end of the stage run,
    // e.g. query the DB size, or any table data.
    #[arg(long, short)]
    commit: bool,
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(self) -> eyre::Result<()> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        fdlimit::raise_fd_limit();

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let config_path = self.config.clone().unwrap_or(data_dir.config_path());

        let config: Config = confy::load_path(config_path).unwrap_or_default();
        info!(target: "reth::cli", "reth {} starting stage {:?}", SHORT_VERSION, self.stage);

        // use the overridden db path if specified
        let db_path = data_dir.db_path();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(db_path)?);
        let mut tx = Transaction::new(db.as_ref())?;

        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", "Starting metrics endpoint at {}", listen_addr);
            prometheus_exporter::initialize_with_db_metrics(listen_addr, Arc::clone(&db)).await?;
        }

        let batch_size = self.batch_size.unwrap_or(self.to - self.from + 1);

        let (mut exec_stage, mut unwind_stage): (Box<dyn Stage<_>>, Option<Box<dyn Stage<_>>>) =
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

                    let network_secret_path = self
                        .network
                        .p2p_secret_key
                        .clone()
                        .unwrap_or_else(|| data_dir.p2p_secret_path());
                    let p2p_secret_key = get_secret_key(&network_secret_path)?;

                    let default_peers_path = data_dir.known_peers_path();

                    let network = self
                        .network
                        .network_config(
                            &config,
                            self.chain.clone(),
                            p2p_secret_key,
                            default_peers_path,
                        )
                        .build(Arc::new(ShareableDatabase::new(db.clone(), self.chain.clone())))
                        .start_network()
                        .await?;
                    let fetch_client = Arc::new(network.fetch_client().await?);

                    let stage = BodyStage {
                        downloader: BodiesDownloaderBuilder::default()
                            .with_stream_batch_size(batch_size as usize)
                            .with_request_limit(config.stages.bodies.downloader_request_limit)
                            .with_max_buffered_blocks(
                                config.stages.bodies.downloader_max_buffered_blocks,
                            )
                            .with_concurrent_requests_range(
                                config.stages.bodies.downloader_min_concurrent_requests..=
                                    config.stages.bodies.downloader_max_concurrent_requests,
                            )
                            .build(fetch_client, consensus.clone(), db.clone()),
                        consensus: consensus.clone(),
                    };

                    (Box::new(stage), None)
                }
                StageEnum::Senders => (Box::new(SenderRecoveryStage::new(batch_size)), None),
                StageEnum::Execution => {
                    let factory = reth_revm::Factory::new(self.chain.clone());
                    (
                        Box::new(ExecutionStage::new(
                            factory,
                            ExecutionStageThresholds {
                                max_blocks: Some(batch_size),
                                max_changes: None,
                                max_changesets: None,
                            },
                        )),
                        None,
                    )
                }
                StageEnum::TxLookup => (Box::new(TransactionLookupStage::new(batch_size)), None),
                StageEnum::AccountHashing => {
                    (Box::new(AccountHashingStage::new(1, batch_size)), None)
                }
                StageEnum::StorageHashing => {
                    (Box::new(StorageHashingStage::new(1, batch_size)), None)
                }
                StageEnum::Merkle => (
                    Box::new(MerkleStage::default_execution()),
                    Some(Box::new(MerkleStage::default_unwind())),
                ),
                StageEnum::AccountHistory => (Box::<IndexAccountHistoryStage>::default(), None),
                StageEnum::StorageHistory => (Box::<IndexStorageHistoryStage>::default(), None),
                _ => return Ok(()),
            };
        if let Some(unwind_stage) = &unwind_stage {
            assert!(exec_stage.type_id() == unwind_stage.type_id());
        }

        let checkpoint = get_stage_checkpoint(tx.deref(), exec_stage.id())?.unwrap_or_default();

        let unwind_stage = unwind_stage.as_mut().unwrap_or(&mut exec_stage);

        let mut unwind = UnwindInput {
            checkpoint: checkpoint.with_block_number(self.to),
            unwind_to: self.from,
            bad_block: None,
        };

        if !self.skip_unwind {
            while unwind.checkpoint.block_number > self.from {
                let unwind_output = unwind_stage.unwind(&mut tx, unwind).await?;
                unwind.checkpoint = unwind_output.checkpoint;
            }
        }

        let mut input = ExecInput {
            previous_stage: Some((
                StageId::Other("No Previous Stage"),
                StageCheckpoint::new(self.to),
            )),
            checkpoint: Some(checkpoint.with_block_number(self.from)),
        };

        while let ExecOutput { checkpoint: stage_progress, done: false } =
            exec_stage.execute(&mut tx, input).await?
        {
            input.checkpoint = Some(stage_progress);

            if self.commit {
                tx.commit()?;
            }
        }

        Ok(())
    }
}
