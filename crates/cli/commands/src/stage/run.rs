//! Main `stage` command
//!
//! Stage debugging tool

use crate::common::{AccessRights, Environment, EnvironmentArgs};
use alloy_eips::BlockHashOrNumber;
use clap::Parser;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_cli_util::get_secret_key;
use reth_config::config::{HashingConfig, SenderRecoveryConfig, TransactionLookupConfig};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_exex::ExExManagerHandle;
use reth_network::BlockDownloaderProvider;
use reth_network_p2p::HeadersClient;
use reth_node_builder::NodeTypesWithEngine;
use reth_node_core::{
    args::{NetworkArgs, StageEnum},
    version::{
        BUILD_PROFILE_NAME, CARGO_PKG_VERSION, VERGEN_BUILD_TIMESTAMP, VERGEN_CARGO_FEATURES,
        VERGEN_CARGO_TARGET_TRIPLE, VERGEN_GIT_SHA,
    },
};
use reth_node_metrics::{
    chain::ChainSpecInfo,
    hooks::Hooks,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
use reth_provider::{
    writer::UnifiedStorageWriter, ChainSpecProvider, DatabaseProviderFactory,
    StageCheckpointReader, StageCheckpointWriter, StaticFileProviderFactory,
};
use reth_stages::{
    stages::{
        AccountHashingStage, BodyStage, ExecutionStage, HeaderStage, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TransactionLookupStage,
    },
    ExecInput, ExecOutput, ExecutionStageThresholds, Stage, StageError, StageExt, UnwindInput,
    UnwindOutput,
};
use std::{any::Any, net::SocketAddr, sync::Arc, time::Instant};
use tokio::sync::watch;
use tracing::*;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET")]
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

    /// Commits the changes in the database. WARNING: potentially destructive.
    ///
    /// Useful when you want to run diagnostics on the database.
    // TODO: We should consider allowing to run hooks at the end of the stage run,
    // e.g. query the DB size, or any table data.
    #[arg(long, short)]
    commit: bool,

    /// Save stage checkpoints
    #[arg(long)]
    checkpoints: bool,

    #[command(flatten)]
    network: NetworkArgs,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `stage` command
    pub async fn execute<N, E, F>(self, ctx: CliContext, executor: F) -> eyre::Result<()>
    where
        N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>,
        E: BlockExecutorProvider,
        F: FnOnce(Arc<C::ChainSpec>) -> E,
    {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        let _ = fdlimit::raise_fd_limit();

        let Environment { provider_factory, config, data_dir } =
            self.env.init::<N>(AccessRights::RW)?;

        let mut provider_rw = provider_factory.database_provider_rw()?;

        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", "Starting metrics endpoint at {}", listen_addr);
            let config = MetricServerConfig::new(
                listen_addr,
                VersionInfo {
                    version: CARGO_PKG_VERSION,
                    build_timestamp: VERGEN_BUILD_TIMESTAMP,
                    cargo_features: VERGEN_CARGO_FEATURES,
                    git_sha: VERGEN_GIT_SHA,
                    target_triple: VERGEN_CARGO_TARGET_TRIPLE,
                    build_profile: BUILD_PROFILE_NAME,
                },
                ChainSpecInfo { name: provider_factory.chain_spec().chain().to_string() },
                ctx.task_executor,
                Hooks::new(
                    provider_factory.db_ref().clone(),
                    provider_factory.static_file_provider(),
                ),
            );

            MetricServer::new(config).serve().await?;
        }

        let batch_size = self.batch_size.unwrap_or(self.to.saturating_sub(self.from) + 1);

        let etl_config = config.stages.etl.clone();
        let prune_modes = config.prune.clone().map(|prune| prune.segments).unwrap_or_default();

        let (mut exec_stage, mut unwind_stage): (Box<dyn Stage<_>>, Option<Box<dyn Stage<_>>>) =
            match self.stage {
                StageEnum::Headers => {
                    let consensus =
                        Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));

                    let network_secret_path = self
                        .network
                        .p2p_secret_key
                        .clone()
                        .unwrap_or_else(|| data_dir.p2p_secret());
                    let p2p_secret_key = get_secret_key(&network_secret_path)?;

                    let default_peers_path = data_dir.known_peers();

                    let network = self
                        .network
                        .network_config(
                            &config,
                            provider_factory.chain_spec(),
                            p2p_secret_key,
                            default_peers_path,
                        )
                        .build(provider_factory.clone())
                        .start_network()
                        .await?;
                    let fetch_client = Arc::new(network.fetch_client().await?);

                    // Use `to` as the tip for the stage
                    let tip = fetch_client
                        .get_header(BlockHashOrNumber::Number(self.to))
                        .await?
                        .into_data()
                        .ok_or(StageError::MissingSyncGap)?;
                    let (_, rx) = watch::channel(tip.hash_slow());

                    (
                        Box::new(HeaderStage::new(
                            provider_factory.clone(),
                            ReverseHeadersDownloaderBuilder::new(config.stages.headers)
                                .build(fetch_client, consensus.clone()),
                            rx,
                            consensus,
                            etl_config,
                        )),
                        None,
                    )
                }
                StageEnum::Bodies => {
                    let consensus =
                        Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));

                    let mut config = config;
                    config.peers.trusted_nodes_only = self.network.trusted_only;
                    config.peers.trusted_nodes.extend(self.network.trusted_peers.clone());

                    let network_secret_path = self
                        .network
                        .p2p_secret_key
                        .clone()
                        .unwrap_or_else(|| data_dir.p2p_secret());
                    let p2p_secret_key = get_secret_key(&network_secret_path)?;

                    let default_peers_path = data_dir.known_peers();

                    let network = self
                        .network
                        .network_config(
                            &config,
                            provider_factory.chain_spec(),
                            p2p_secret_key,
                            default_peers_path,
                        )
                        .build(provider_factory.clone())
                        .start_network()
                        .await?;
                    let fetch_client = Arc::new(network.fetch_client().await?);

                    let stage = BodyStage::new(
                        BodiesDownloaderBuilder::default()
                            .with_stream_batch_size(batch_size as usize)
                            .with_request_limit(config.stages.bodies.downloader_request_limit)
                            .with_max_buffered_blocks_size_bytes(
                                config.stages.bodies.downloader_max_buffered_blocks_size_bytes,
                            )
                            .with_concurrent_requests_range(
                                config.stages.bodies.downloader_min_concurrent_requests..=
                                    config.stages.bodies.downloader_max_concurrent_requests,
                            )
                            .build(fetch_client, consensus.clone(), provider_factory.clone()),
                    );
                    (Box::new(stage), None)
                }
                StageEnum::Senders => (
                    Box::new(SenderRecoveryStage::new(SenderRecoveryConfig {
                        commit_threshold: batch_size,
                    })),
                    None,
                ),
                StageEnum::Execution => (
                    Box::new(ExecutionStage::new(
                        executor(provider_factory.chain_spec()),
                        ExecutionStageThresholds {
                            max_blocks: Some(batch_size),
                            max_changes: None,
                            max_cumulative_gas: None,
                            max_duration: None,
                        },
                        config.stages.merkle.clean_threshold,
                        prune_modes,
                        ExExManagerHandle::empty(),
                    )),
                    None,
                ),
                StageEnum::TxLookup => (
                    Box::new(TransactionLookupStage::new(
                        TransactionLookupConfig { chunk_size: batch_size },
                        etl_config,
                        prune_modes.transaction_lookup,
                    )),
                    None,
                ),
                StageEnum::AccountHashing => (
                    Box::new(AccountHashingStage::new(
                        HashingConfig { clean_threshold: 1, commit_threshold: batch_size },
                        etl_config,
                    )),
                    None,
                ),
                StageEnum::StorageHashing => (
                    Box::new(StorageHashingStage::new(
                        HashingConfig { clean_threshold: 1, commit_threshold: batch_size },
                        etl_config,
                    )),
                    None,
                ),
                StageEnum::Merkle => (
                    Box::new(MerkleStage::new_execution(config.stages.merkle.clean_threshold)),
                    Some(Box::new(MerkleStage::default_unwind())),
                ),
                StageEnum::AccountHistory => (
                    Box::new(IndexAccountHistoryStage::new(
                        config.stages.index_account_history,
                        etl_config,
                        prune_modes.account_history,
                    )),
                    None,
                ),
                StageEnum::StorageHistory => (
                    Box::new(IndexStorageHistoryStage::new(
                        config.stages.index_storage_history,
                        etl_config,
                        prune_modes.storage_history,
                    )),
                    None,
                ),
                _ => return Ok(()),
            };
        if let Some(unwind_stage) = &unwind_stage {
            assert_eq!((*exec_stage).type_id(), (**unwind_stage).type_id());
        }

        let checkpoint = provider_rw.get_stage_checkpoint(exec_stage.id())?.unwrap_or_default();

        let unwind_stage = unwind_stage.as_mut().unwrap_or(&mut exec_stage);

        let mut unwind = UnwindInput {
            checkpoint: checkpoint.with_block_number(self.to),
            unwind_to: self.from,
            bad_block: None,
        };

        if !self.skip_unwind {
            while unwind.checkpoint.block_number > self.from {
                let UnwindOutput { checkpoint } = unwind_stage.unwind(&provider_rw, unwind)?;
                unwind.checkpoint = checkpoint;

                if self.checkpoints {
                    provider_rw.save_stage_checkpoint(unwind_stage.id(), checkpoint)?;
                }

                if self.commit {
                    UnifiedStorageWriter::commit_unwind(
                        provider_rw,
                        provider_factory.static_file_provider(),
                    )?;
                    provider_rw = provider_factory.database_provider_rw()?;
                }
            }
        }

        let mut input = ExecInput {
            target: Some(self.to),
            checkpoint: Some(checkpoint.with_block_number(self.from)),
        };

        let start = Instant::now();
        info!(target: "reth::cli", stage = %self.stage, "Executing stage");
        loop {
            exec_stage.execute_ready(input).await?;
            let ExecOutput { checkpoint, done } = exec_stage.execute(&provider_rw, input)?;

            input.checkpoint = Some(checkpoint);

            if self.checkpoints {
                provider_rw.save_stage_checkpoint(exec_stage.id(), checkpoint)?;
            }
            if self.commit {
                UnifiedStorageWriter::commit(provider_rw, provider_factory.static_file_provider())?;
                provider_rw = provider_factory.database_provider_rw()?;
            }

            if done {
                break
            }
        }
        info!(target: "reth::cli", stage = %self.stage, time = ?start.elapsed(), "Finished stage");

        Ok(())
    }
}
