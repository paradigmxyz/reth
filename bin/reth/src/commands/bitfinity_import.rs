//! Command that initializes the node by importing a chain from a remote EVM node.

use crate::{dirs::DataDirPath, version::SHORT_VERSION};
use bitfinity_block_validator::BitfinityBlockValidator;
use candid::Principal;
use evm_canister_client::{ic_agent::{identity::AnonymousIdentity, Agent}, EvmCanisterClient, IcAgentClient};
use futures::{Stream, StreamExt};
use lightspeed_scheduler::{job::Job, scheduler::Scheduler, JobExecutor};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::ChainSpec;
use reth_config::{config::EtlConfig, Config};
use reth_db::DatabaseEnv;

use alloy_primitives::B256;
use reth_consensus::Consensus;
use reth_downloaders::{
    bitfinity_evm_client::{BitfinityEvmClient, CertificateCheckSettings, RpcClientConfig},
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_exex::ExExManagerHandle;
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_core::{args::{BitfinityImportArgs, IC_MAINNET_KEY, IC_MAINNET_URL}, dirs::ChainPath};
use reth_node_ethereum::{EthExecutorProvider, EthereumNode};
use reth_node_events::node::NodeEvent;
use reth_primitives::{EthPrimitives, SealedHeader};
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, CanonChainTracker, ChainSpecProvider,
    DatabaseProviderFactory, HeaderProvider, ProviderError, ProviderFactory,
};
use reth_prune::PruneModes;
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, SenderRecoveryStage},
    ExecutionStageThresholds, Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Syncs RLP encoded blocks from a file.
#[derive(Clone)]
pub struct BitfinityImportCommand {
    config: Config,

    datadir: ChainPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    chain: Arc<ChainSpec>,

    /// Bitfinity Related Args
    bitfinity: BitfinityImportArgs,

    provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,

    blockchain_provider: BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
}

/// Manually implement `Debug` for `ImportCommand` because `BlockchainProvider` doesn't implement
/// it.
impl std::fmt::Debug for BitfinityImportCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportCommand")
            .field("config", &self.config)
            .field("datadir", &self.datadir)
            .field("chain", &self.chain)
            .field("bitfinity", &self.bitfinity)
            .finish()
    }
}

type TypedPipeline = Pipeline<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;

impl BitfinityImportCommand {
    /// Create a new `ImportCommand` with the given arguments.
    pub fn new(
        config: Option<PathBuf>,
        datadir: ChainPath<DataDirPath>,
        chain: Arc<ChainSpec>,
        bitfinity: BitfinityImportArgs,
        provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
        blockchain_provider: BlockchainProvider<
            NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>,
        >,
    ) -> Self {
        // add network name to data dir
        let config_path = config.unwrap_or_else(|| datadir.config());

        info!(target: "reth::cli - BitfinityImportCommand", path = ?config_path, "Configuration loaded");
        let mut config = Config::from_path(config_path)
            .expect("Failed to load BitfinityImportCommand configuration");

        // Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
        if config.stages.etl.dir.is_none() {
            config.stages.etl.dir = Some(EtlConfig::from_datadir(datadir.data_dir()));
        }

        Self { config, datadir, chain, bitfinity, provider_factory, blockchain_provider }
    }

    /// Schedule the import job and return a handle to it.
    pub async fn schedule_execution(
        self,
    ) -> eyre::Result<(JobExecutor, tokio::task::JoinHandle<()>)> {
        info!(target: "reth::cli - BitfinityImportCommand", "reth {} starting", SHORT_VERSION);

        let job_executor = JobExecutor::new_with_local_tz();

        // Schedule the import job
        {
            let interval = Duration::from_secs(self.bitfinity.import_interval);
            job_executor
                .add_job_with_scheduler(
                    Scheduler::Interval { interval_duration: interval, execute_at_startup: true },
                    Job::new("import", "block importer", None, move || {
                        let import = self.clone();
                        Box::pin(async move {
                            import.single_execution().await?;
                            import.update_chain_info()?;
                            Ok(())
                        })
                    }),
                )
                .await;
        }

        let job_handle = job_executor.run().await?;
        Ok((job_executor, job_handle))
    }

    /// Execute the import job.
    async fn single_execution(&self) -> eyre::Result<()> {
        let consensus = Arc::new(EthBeaconConsensus::new(self.chain.clone()));
        debug!(target: "reth::cli - BitfinityImportCommand", "Consensus engine initialized");
        let provider_factory = self.provider_factory.clone();

        // Get the local block number
        let start_block = provider_factory.provider()?.last_block_number()? + 1;

        debug!(target: "reth::cli - BitfinityImportCommand", "Starting block: {}", start_block);

        let rpc_config = RpcClientConfig {
            primary_url: self.bitfinity.rpc_url.clone(),
            backup_url: self.bitfinity.backup_rpc_url.clone(),
            max_retries: self.bitfinity.max_retries,
            retry_delay: Duration::from_secs(self.bitfinity.retry_delay_secs),
            max_block_age_secs: Duration::from_secs(self.bitfinity.max_block_age_secs),
        };

        let remote_client = Arc::new(
            BitfinityEvmClient::from_rpc_url(
                rpc_config,
                start_block,
                self.bitfinity.end_block,
                self.bitfinity.batch_size,
                self.bitfinity.max_fetch_blocks,
                Some(CertificateCheckSettings {
                    evmc_principal: self.bitfinity.evmc_principal.clone(),
                    ic_root_key: self.bitfinity.ic_root_key.clone(),
                }),
                self.bitfinity.check_evm_state_before_importing,
            )
            .await?,
        );

        // override the tip
        let safe_block = if let Some(safe_block) = remote_client.safe_block() {
            safe_block
        } else {
            debug!(target: "reth::cli - BitfinityImportCommand", "No safe block found, skipping import");
            return Ok(());
        };

        self.import_to_block(
            safe_block,
            remote_client.clone(),
            provider_factory.clone(),
            consensus.clone(),
        )
        .await?;

        if self.bitfinity.validate_unsafe_blocks {
            let Some(mut tip) = remote_client.tip() else {
                warn!(target: "reth::cli - BitfinityImportCommand", "Cannot find block for validation. Skipping.");
                return Ok(());
            };

            while tip != safe_block {
                match self.validate_block(&tip, remote_client.clone(), provider_factory.clone()).await {
                    Ok(_) => {
                        self.import_to_block(tip, remote_client, provider_factory, consensus)
                            .await?;
                        break;
                    }

                    Err(err) => {
                        warn!(target: "reth::cli - BitfinityImportCommand", "Failed to validate block {}: {}", tip, err);

                        if let Some(parent) = remote_client.parent(&tip) {
                            tip = parent;
                        } else {
                            warn!(target: "reth::cli - BitfinityImportCommand", "Cannot find a parent block for {}", tip);
                            break;
                        }
                    }
                }
            }
        }

        info!(target: "reth::cli - BitfinityImportCommand", "Finishing up");
        Ok(())
    }

    async fn validate_block(
        &self,
        block: &B256,
        remote_client: Arc<BitfinityEvmClient>,
        provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<()> {
        let agent = Agent::builder().with_identity(AnonymousIdentity).with_url(self.bitfinity.evm_network.clone()).build()?;
        if self.bitfinity.evm_network == IC_MAINNET_URL {
            let key = hex::decode(IC_MAINNET_KEY)?;
            agent.set_root_key(key);
        } else {
            agent.fetch_root_key().await?;
        }

        let evm_principal = Principal::from_text(&self.bitfinity.evmc_principal)?;
        let agent_client = IcAgentClient::with_agent(evm_principal, agent);
        let canister_client = EvmCanisterClient::new(agent_client);

        let validator = BitfinityBlockValidator::new(canister_client, provider_factory);
        let blocks = remote_client.unsafe_blocks(block)?;

        validator.validate_blocks(&blocks).await
    }

    /// Imports the blocks up to the given block hash of the `remove_client`.
    async fn import_to_block(
        &self,
        new_tip: B256,
        remote_client: Arc<BitfinityEvmClient>,
        provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
        consensus: Arc<EthBeaconConsensus<ChainSpec>>,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli - BitfinityImportCommand", "Chain blocks imported");

        let (mut pipeline, _events) = self.build_import_pipeline(
            &self.config,
            provider_factory.clone(),
            &consensus,
            remote_client,
            StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
        )?;

        // override the tip
        pipeline.set_tip(new_tip);
        debug!(target: "reth::cli - BitfinityImportCommand", ?new_tip, "Tip manually set");

        // Run pipeline
        debug!(target: "reth::cli - BitfinityImportCommand", "Starting sync pipeline");
        pipeline.run().await?;

        Ok(())
    }

    /// Update the chain info tracker with the latest header from the database.
    fn update_chain_info(&self) -> eyre::Result<()> {
        let provider = self.blockchain_provider.database_provider_ro()?;
        let chain_info = provider.chain_info()?;

        match provider.header_by_number(chain_info.best_number)? {
            Some(header) => {
                let sealed_header = header.seal(chain_info.best_hash);
                let hash = sealed_header.seal();
                let sealed_header = SealedHeader::new(sealed_header.into_inner(), hash);
                self.blockchain_provider.set_canonical_head(sealed_header.clone());
                self.blockchain_provider.set_finalized(sealed_header.clone());
                self.blockchain_provider.set_safe(sealed_header);
                Ok(())
            }
            None => Err(ProviderError::HeaderNotFound(chain_info.best_number.into()))?,
        }
    }

    fn build_import_pipeline<C>(
        &self,
        config: &Config,
        provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
        consensus: &Arc<C>,
        remote_client: Arc<BitfinityEvmClient>,
        static_file_producer: StaticFileProducer<
            ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
        >,
    ) -> eyre::Result<(TypedPipeline, impl Stream<Item = NodeEvent<EthPrimitives>>)>
    where
        C: Consensus + 'static,
    {
        if !remote_client.has_canonical_blocks() {
            eyre::bail!("unable to import non canonical blocks");
        }

        let header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
            .build(remote_client.clone(), consensus.clone())
            .into_task();

        let body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies)
            .build(remote_client.clone(), consensus.clone(), provider_factory.clone())
            .into_task();

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        let executor = EthExecutorProvider::ethereum(provider_factory.chain_spec());

        let max_block = remote_client.max_block().unwrap_or(0);
        let pipeline =
            Pipeline::<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>::builder()
                .with_tip_sender(tip_tx)
                // we want to sync all blocks the file client provides or 0 if empty
                .with_max_block(max_block)
                .add_stages(
                    DefaultStages::new(
                        provider_factory.clone(),
                        tip_rx,
                        consensus.clone(),
                        header_downloader,
                        body_downloader,
                        executor.clone(),
                        config.stages.clone(),
                        PruneModes::default(),
                    )
                    .set(SenderRecoveryStage {
                        commit_threshold: config.stages.sender_recovery.commit_threshold,
                    })
                    .set(ExecutionStage::new(
                        executor,
                        ExecutionStageThresholds {
                            max_blocks: config.stages.execution.max_blocks,
                            max_changes: config.stages.execution.max_changes,
                            max_cumulative_gas: config.stages.execution.max_cumulative_gas,
                            max_duration: config.stages.execution.max_duration,
                        },
                        config
                            .stages
                            .merkle
                            .clean_threshold
                            .max(config.stages.account_hashing.clean_threshold)
                            .max(config.stages.storage_hashing.clean_threshold),
                        config.prune.clone().map(|prune| prune.segments).unwrap_or_default(),
                        ExExManagerHandle::empty(),
                    )),
                )
                .build(provider_factory, static_file_producer);

        let events = pipeline.events().map(Into::into);

        Ok((pipeline, events))
    }
}
