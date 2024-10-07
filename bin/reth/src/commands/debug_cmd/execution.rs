//! Command for debugging execution.

use crate::{args::NetworkArgs, utils::get_single_header};
use alloy_primitives::{BlockNumber, B256};
use clap::Parser;
use futures::{stream::select as stream_select, StreamExt};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_cli_runner::CliContext;
use reth_cli_util::get_secret_key;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::DatabaseEnv;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_exex::ExExManagerHandle;
use reth_network::{BlockDownloaderProvider, NetworkEventListenerProvider, NetworkHandle};
use reth_network_api::NetworkInfo;
use reth_network_p2p::{headers::client::HeadersClient, BlockClient};
use reth_node_api::{NodeTypesWithDB, NodeTypesWithDBAdapter, NodeTypesWithEngine};
use reth_node_ethereum::EthExecutorProvider;
use reth_primitives::BlockHashOrNumber;
use reth_provider::{
    BlockExecutionWriter, ChainSpecProvider, ProviderFactory, StageCheckpointReader,
};
use reth_prune::PruneModes;
use reth_stages::{
    sets::DefaultStages, stages::ExecutionStage, ExecutionStageThresholds, Pipeline, StageId,
    StageSet,
};
use reth_static_file::StaticFileProducer;
use reth_tasks::TaskExecutor;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::*;

/// `reth debug execution` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(flatten)]
    network: NetworkArgs,

    /// The maximum block height.
    #[arg(long)]
    pub to: u64,

    /// The block interval for sync and unwind.
    /// Defaults to `1000`.
    #[arg(long, default_value = "1000")]
    pub interval: u64,
}

impl<C: ChainSpecParser<ChainSpec = ChainSpec>> Command<C> {
    fn build_pipeline<N: NodeTypesWithDB<ChainSpec = C::ChainSpec>, Client>(
        &self,
        config: &Config,
        client: Client,
        consensus: Arc<dyn Consensus>,
        provider_factory: ProviderFactory<N>,
        task_executor: &TaskExecutor,
        static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    ) -> eyre::Result<Pipeline<N>>
    where
        Client: BlockClient + 'static,
    {
        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies)
            .build(client, Arc::clone(&consensus), provider_factory.clone())
            .into_task_with(task_executor);

        let stage_conf = &config.stages;
        let prune_modes = config.prune.clone().map(|prune| prune.segments).unwrap_or_default();

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        let executor = EthExecutorProvider::ethereum(provider_factory.chain_spec());

        let pipeline = Pipeline::<N>::builder()
            .with_tip_sender(tip_tx)
            .add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    tip_rx,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    executor.clone(),
                    stage_conf.clone(),
                    prune_modes.clone(),
                )
                .set(ExecutionStage::new(
                    executor,
                    ExecutionStageThresholds {
                        max_blocks: None,
                        max_changes: None,
                        max_cumulative_gas: None,
                        max_duration: None,
                    },
                    stage_conf.execution_external_clean_threshold(),
                    prune_modes,
                    ExExManagerHandle::empty(),
                )),
            )
            .build(provider_factory, static_file_producer);

        Ok(pipeline)
    }

    async fn build_network<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        &self,
        config: &Config,
        task_executor: TaskExecutor,
        provider_factory: ProviderFactory<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
        network_secret_path: PathBuf,
        default_peers_path: PathBuf,
    ) -> eyre::Result<NetworkHandle> {
        let secret_key = get_secret_key(&network_secret_path)?;
        let network = self
            .network
            .network_config(config, provider_factory.chain_spec(), secret_key, default_peers_path)
            .with_task_executor(Box::new(task_executor))
            .build(provider_factory)
            .start_network()
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        Ok(network)
    }

    async fn fetch_block_hash<Client: HeadersClient>(
        &self,
        client: Client,
        block: BlockNumber,
    ) -> eyre::Result<B256> {
        info!(target: "reth::cli", ?block, "Fetching block from the network.");
        loop {
            match get_single_header(&client, BlockHashOrNumber::Number(block)).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?block, "Successfully fetched block");
                    return Ok(tip_header.hash())
                }
                Err(error) => {
                    error!(target: "reth::cli", ?block, %error, "Failed to fetch the block. Retrying...");
                }
            }
        }
    }

    /// Execute `execution-debug` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        let Environment { provider_factory, config, data_dir } =
            self.env.init::<N>(AccessRights::RW)?;

        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));

        // Configure and build network
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret());
        let network = self
            .build_network(
                &config,
                ctx.task_executor.clone(),
                provider_factory.clone(),
                network_secret_path,
                data_dir.known_peers(),
            )
            .await?;

        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

        // Configure the pipeline
        let fetch_client = network.fetch_client().await?;
        let mut pipeline = self.build_pipeline(
            &config,
            fetch_client.clone(),
            Arc::clone(&consensus),
            provider_factory.clone(),
            &ctx.task_executor,
            static_file_producer,
        )?;

        let provider = provider_factory.provider()?;

        let latest_block_number =
            provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        if latest_block_number.unwrap_or_default() >= self.to {
            info!(target: "reth::cli", latest = latest_block_number, "Nothing to run");
            return Ok(())
        }

        let pipeline_events = pipeline.events();
        let events = stream_select(
            network.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
        );
        ctx.task_executor.spawn_critical(
            "events task",
            reth_node_events::node::handle_events(
                Some(Box::new(network)),
                latest_block_number,
                events,
            ),
        );

        let mut current_max_block = latest_block_number.unwrap_or_default();
        while current_max_block < self.to {
            let next_block = current_max_block + 1;
            let target_block = self.to.min(current_max_block + self.interval);
            let target_block_hash =
                self.fetch_block_hash(fetch_client.clone(), target_block).await?;

            // Run the pipeline
            info!(target: "reth::cli", from = next_block, to = target_block, tip = ?target_block_hash, "Starting pipeline");
            pipeline.set_tip(target_block_hash);
            let result = pipeline.run_loop().await?;
            trace!(target: "reth::cli", from = next_block, to = target_block, tip = ?target_block_hash, ?result, "Pipeline finished");

            // Unwind the pipeline without committing.
            {
                provider_factory
                    .provider_rw()?
                    .take_block_and_execution_range(next_block..=target_block)?;
            }

            // Update latest block
            current_max_block = target_block;
        }

        Ok(())
    }
}
