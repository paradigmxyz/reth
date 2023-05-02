use crate::{
    dirs::{ConfigPath, DbPath, MaybePlatformPath, PlatformPath},
    node::events::{handle_events, NodeEvent},
};
use clap::{crate_version, Parser};
use eyre::Context;
use futures::{Stream, StreamExt};
use reth_beacon_consensus::BeaconConsensus;
use reth_db::mdbx::{Env, WriteMap};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder, test_utils::FileClient,
};
use reth_interfaces::{
    consensus::Consensus, p2p::headers::client::NoopStatusUpdater, sync::SyncStateUpdater,
};
use reth_primitives::{Chain, ChainSpec, H256};
use reth_staged_sync::{
    utils::{
        chainspec::genesis_value_parser,
        init::{init_db, init_genesis},
    },
    Config,
};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, HeaderSyncMode, SenderRecoveryStage, TotalDifficultyStage},
};
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Debug, Parser)]
pub struct ImportCommand {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: MaybePlatformPath<ConfigPath>,

    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: MaybePlatformPath<DbPath>,

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
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// The path to a block file for import.
    ///
    /// The online stages (headers and bodies) are replaced by a file import, after which the
    /// remaining stages are executed.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PlatformPath<ConfigPath>,
}

impl ImportCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", crate_version!());

        let config: Config = self.load_config_with_chain(self.chain.chain)?;
        info!(target: "reth::cli", path = %self.config.unwrap_or_chain_default(self.chain.chain), "Configuration loaded");

        // add network name to db directory
        let db_path = self.db.unwrap_or_chain_default(self.chain.chain);

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(db_path)?);
        info!(target: "reth::cli", "Database opened");

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(db.clone(), self.chain.clone())?;

        let consensus = Arc::new(BeaconConsensus::new(self.chain.clone()));
        info!(target: "reth::cli", "Consensus engine initialized");

        // create a new FileClient
        info!(target: "reth::cli", "Importing chain file");
        let file_client = Arc::new(FileClient::new(&self.path).await?);

        // override the tip
        let tip = file_client.tip().expect("file client has no tip");
        info!(target: "reth::cli", "Chain file imported");

        let (mut pipeline, events) =
            self.build_import_pipeline(config, db.clone(), &consensus, file_client).await?;

        // override the tip
        pipeline.set_tip(tip);
        debug!(target: "reth::cli", ?tip, "Tip manually set");

        tokio::spawn(handle_events(None, events));

        // Run pipeline
        info!(target: "reth::cli", "Starting sync pipeline");
        tokio::select! {
            res = pipeline.run(db.clone()) => res?,
            _ = tokio::signal::ctrl_c() => {},
        };

        info!(target: "reth::cli", "Finishing up");
        Ok(())
    }

    async fn build_import_pipeline<C>(
        &self,
        config: Config,
        db: Arc<Env<WriteMap>>,
        consensus: &Arc<C>,
        file_client: Arc<FileClient>,
    ) -> eyre::Result<(Pipeline<Env<WriteMap>, impl SyncStateUpdater>, impl Stream<Item = NodeEvent>)>
    where
        C: Consensus + 'static,
    {
        if !file_client.has_canonical_blocks() {
            eyre::bail!("unable to import non canonical blocks");
        }

        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(file_client.clone(), consensus.clone())
            .into_task();

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(file_client.clone(), consensus.clone(), db)
            .into_task();

        let (tip_tx, tip_rx) = watch::channel(H256::zero());
        let factory = reth_revm::Factory::new(self.chain.clone());

        let mut pipeline = Pipeline::builder()
            .with_tip_sender(tip_tx)
            // we want to sync all blocks the file client provides or 0 if empty
            .with_max_block(file_client.max_block().unwrap_or(0))
            .with_sync_state_updater(file_client)
            .add_stages(
                DefaultStages::new(
                    HeaderSyncMode::Tip(tip_rx),
                    consensus.clone(),
                    header_downloader,
                    body_downloader,
                    NoopStatusUpdater::default(),
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus.clone())
                        .with_commit_threshold(config.stages.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: config.stages.sender_recovery.commit_threshold,
                })
                .set(ExecutionStage::new(factory, config.stages.execution.commit_threshold)),
            )
            .build();

        let events = pipeline.events().map(Into::into);

        Ok((pipeline, events))
    }

    /// Loads the reth config based on the intended chain
    fn load_config_with_chain(&self, chain: Chain) -> eyre::Result<Config> {
        // add network name to config directory
        let config_path = self.config.unwrap_or_chain_default(chain);
        confy::load_path::<Config>(config_path.clone())
            .wrap_err_with(|| format!("Could not load config file {}", config_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_common_import_command_chain_args() {
        for chain in ["mainnet", "sepolia", "goerli"] {
            let args: ImportCommand = ImportCommand::parse_from(["reth", "--chain", chain, "."]);
            assert_eq!(args.chain.chain, chain.parse().unwrap());
        }
    }
}
