//! Unwinding a certain block range

use clap::{Parser, Subcommand};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::{database::Database, open_db};
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_exex::ExExManagerHandle;
use reth_node_core::args::NetworkArgs;
use reth_primitives::{BlockHashOrNumber, ChainSpec, PruneModes, B256};
use reth_provider::{
    BlockExecutionWriter, BlockNumReader, ChainSpecProvider, HeaderSyncMode, ProviderFactory,
    StaticFileProviderFactory,
};
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

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    macros::block_executor,
};

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
        let db_path = data_dir.db();
        if !db_path.exists() {
            eyre::bail!("Database {db_path:?} does not exist.")
        }
        let config_path = data_dir.config();
        let config: Config = confy::load_path(config_path).unwrap_or_default();

        let db = Arc::new(open_db(db_path.as_ref(), self.db.database_args())?);
        let provider_factory =
            ProviderFactory::new(db, self.chain.clone(), data_dir.static_files())?;

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
            let mut pipeline = self.build_pipeline(config, provider_factory.clone()).await?;

            // Move all applicable data from database to static files.
            pipeline.move_to_static_files()?;

            pipeline.unwind((*range.start()).saturating_sub(1), None)?;
        } else {
            info!(target: "reth::cli", ?range, "Executing a database unwind.");
            let provider = provider_factory.provider_rw()?;

            let _ = provider
                .take_block_and_execution_range(range.clone())
                .map_err(|err| eyre::eyre!("Transaction error on unwind: {err}"))?;

            provider.commit()?;
        }

        println!("Unwound {} blocks", range.count());

        Ok(())
    }

    async fn build_pipeline<DB: Database + 'static>(
        self,
        config: Config,
        provider_factory: ProviderFactory<Arc<DB>>,
    ) -> Result<Pipeline<Arc<DB>>, eyre::Error> {
        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));
        let stage_conf = &config.stages;

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        let executor = block_executor!(provider_factory.chain_spec());

        let header_mode = HeaderSyncMode::Tip(tip_rx);
        let pipeline = Pipeline::builder()
            .with_tip_sender(tip_tx)
            .add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    header_mode,
                    Arc::clone(&consensus),
                    NoopHeaderDownloader::default(),
                    NoopBodiesDownloader::default(),
                    executor.clone(),
                    stage_conf.etl.clone(),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: stage_conf.sender_recovery.commit_threshold,
                })
                .set(ExecutionStage::new(
                    executor,
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
                    ExExManagerHandle::empty(),
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
    /// Unwinds the database from the latest block, until the given block number or hash has been
    /// reached, that block is not included.
    #[command(name = "to-block")]
    ToBlock { target: BlockHashOrNumber },
    /// Unwinds the database from the latest block, until the given number of blocks have been
    /// reached.
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
        if target > last {
            eyre::bail!("Target block number is higher than the latest block number")
        }
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
