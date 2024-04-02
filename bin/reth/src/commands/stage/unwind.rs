//! Unwinding a certain block range

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::{Parser, Subcommand};
use reth_beacon_consensus::BeaconConsensus;
use reth_config::{Config, PruneConfig};
use reth_db::{database::Database, open_db};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::consensus::Consensus;
use reth_node_core::{
    args::{get_secret_key, NetworkArgs},
    dirs::ChainPath,
};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{BlockHashOrNumber, ChainSpec, PruneModes, B256};
use reth_provider::{
    BlockExecutionWriter, BlockNumReader, ChainSpecProvider, HeaderSyncMode, ProviderFactory,
};
use reth_prune::PrunerBuilder;
use reth_stages::{
    sets::DefaultStages,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TransactionLookupStage,
    },
    Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use std::{ops::RangeInclusive, sync::Arc};
use tokio::sync::watch;
use tracing::info;

/// `reth stage unwind` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t, global = true)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser,
        global = true
    )]
    chain: Arc<ChainSpec>,

    #[command(flatten)]
    db: DatabaseArgs,

    #[command(flatten)]
    network: NetworkArgs,

    #[command(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Execute `db stage unwind` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        if !db_path.exists() {
            eyre::bail!("Database {db_path:?} does not exist.")
        }
        let config_path = data_dir.config_path();
        let config: Config = confy::load_path(config_path).unwrap_or_default();

        let db = Arc::new(open_db(db_path.as_ref(), self.db.database_args())?);
        let provider_factory =
            ProviderFactory::new(db, self.chain.clone(), data_dir.static_files_path())?;

        let range = self.command.unwind_range(provider_factory.clone())?;
        if *range.start() == 0 {
            eyre::bail!("Cannot unwind genesis block")
        }

        // Only execute a pipeline unwind if the start of the range overlaps the existing static
        // files. If that's the case, then copy all available data from MDBX to static files, and
        // only then, proceed with the unwind.
        if let Some(highest_static_block) = provider_factory
            .static_file_provider()
            .get_highest_static_files()
            .max()
            .filter(|highest_static_file_block| highest_static_file_block >= range.start())
        {
            info!(target: "reth::cli", ?range, ?highest_static_block, "Executing a pipeline unwind.");
            let mut pipeline =
                self.build_pipeline(data_dir, config, provider_factory.clone()).await?;

            // Move all applicable data from database to static files.
            pipeline.produce_static_files()?;

            // Run the pruner so we don't potentially end up with higher height in the database vs
            // static files.
            let mut pruner = PrunerBuilder::new(PruneConfig::default())
                .prune_delete_limit(usize::MAX)
                .build(provider_factory);
            pruner.run(*range.end())?;

            pipeline.unwind((*range.start()).saturating_sub(1), None)?;
        } else {
            info!(target: "reth::cli", ?range, "Executing a database unwind.");
            let provider = provider_factory.provider_rw()?;

            let _ = provider
                .take_block_and_execution_range(&self.chain, range.clone())
                .map_err(|err| eyre::eyre!("Transaction error on unwind: {err}"))?;

            provider.commit()?;
        }

        println!("Unwound {} blocks", range.count());

        Ok(())
    }

    async fn build_pipeline<DB: Database + 'static>(
        self,
        data_dir: ChainPath<DataDirPath>,
        config: Config,
        provider_factory: ProviderFactory<Arc<DB>>,
    ) -> Result<Pipeline<Arc<DB>>, eyre::Error> {
        // Even though we are not planning to download anything, we need to initialize Body and
        // Header stage with a network client
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());
        let p2p_secret_key = get_secret_key(&network_secret_path)?;
        let default_peers_path = data_dir.known_peers_path();
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

        let consensus: Arc<dyn Consensus> =
            Arc::new(BeaconConsensus::new(provider_factory.chain_spec()));

        // building network downloaders using the fetch client
        let fetch_client = network.fetch_client().await?;
        let header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
            .build(fetch_client.clone(), Arc::clone(&consensus));
        let body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies).build(
            fetch_client,
            Arc::clone(&consensus),
            provider_factory.clone(),
        );
        let stage_conf = &config.stages;

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        let factory = reth_revm::EvmProcessorFactory::new(
            provider_factory.chain_spec(),
            EthEvmConfig::default(),
        );

        let header_mode = HeaderSyncMode::Tip(tip_rx);
        let pipeline = Pipeline::builder()
            .with_tip_sender(tip_tx)
            .add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    header_mode,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    factory.clone(),
                    stage_conf.etl.clone(),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: stage_conf.sender_recovery.commit_threshold,
                })
                .set(ExecutionStage::new(
                    factory,
                    ExecutionStageThresholds {
                        max_blocks: None,
                        max_changes: None,
                        max_cumulative_gas: None,
                        max_duration: None,
                    },
                    stage_conf
                        .merkle
                        .clean_threshold
                        .max(stage_conf.account_hashing.clean_threshold)
                        .max(stage_conf.storage_hashing.clean_threshold),
                    config.prune.clone().map(|prune| prune.segments).unwrap_or_default(),
                ))
                .set(AccountHashingStage::default())
                .set(StorageHashingStage::default())
                .set(MerkleStage::default_unwind())
                .set(TransactionLookupStage::default())
                .set(IndexAccountHistoryStage::default())
                .set(IndexStorageHistoryStage::default()),
            )
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(
                    provider_factory.clone(),
                    provider_factory.static_file_provider(),
                    PruneModes::default(),
                ),
            );
        Ok(pipeline)
    }
}

/// `reth stage unwind` subcommand
#[derive(Subcommand, Debug, Eq, PartialEq)]
enum Subcommands {
    /// Unwinds the database until the given block number (range is inclusive).
    #[command(name = "to-block")]
    ToBlock { target: BlockHashOrNumber },
    /// Unwinds the given number of blocks from the database.
    #[command(name = "num-blocks")]
    NumBlocks { amount: u64 },
}

impl Subcommands {
    /// Returns the block range to unwind.
    ///
    /// This returns an inclusive range: [target..=latest]
    fn unwind_range<DB: Database>(
        &self,
        factory: ProviderFactory<DB>,
    ) -> eyre::Result<RangeInclusive<u64>> {
        let provider = factory.provider()?;
        let last = provider.last_block_number()?;
        let target = match self {
            Subcommands::ToBlock { target } => match target {
                BlockHashOrNumber::Hash(hash) => provider
                    .block_number(*hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => *num,
            },
            Subcommands::NumBlocks { amount } => last.saturating_sub(*amount),
        } + 1;
        Ok(target..=last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unwind() {
        let cmd = Command::parse_from(["reth", "--datadir", "dir", "to-block", "100"]);
        assert_eq!(cmd.command, Subcommands::ToBlock { target: BlockHashOrNumber::Number(100) });

        let cmd = Command::parse_from(["reth", "--datadir", "dir", "num-blocks", "100"]);
        assert_eq!(cmd.command, Subcommands::NumBlocks { amount: 100 });
    }
}
