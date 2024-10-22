//! Command that initializes the node by importing a chain from a file.
use crate::common::{AccessRights, Environment, EnvironmentArgs};
use alloy_primitives::B256;
use clap::Parser;
use futures::{Stream, StreamExt};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::tables;
use reth_db_api::transaction::DbTx;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    file_client::{ChunkedFileReader, FileClient, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE},
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_network_p2p::{
    bodies::downloader::BodyDownloader,
    headers::downloader::{HeaderDownloader, SyncTarget},
};
use reth_node_builder::NodeTypesWithEngine;
use reth_node_core::version::SHORT_VERSION;
use reth_node_events::node::NodeEvent;
use reth_provider::{
    providers::ProviderNodeTypes, BlockNumReader, ChainSpecProvider, HeaderProvider, ProviderError,
    ProviderFactory, StageCheckpointReader,
};
use reth_prune::PruneModes;
use reth_stages::{prelude::*, Pipeline, StageId, StageSet};
use reth_static_file::StaticFileProducer;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, error, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Debug, Parser)]
pub struct ImportCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Disables stages that require state.
    #[arg(long, verbatim_doc_comment)]
    no_state: bool,

    /// Chunk byte length to read from file.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    /// The path to a block file for import.
    ///
    /// The online stages (headers and bodies) are replaced by a file import, after which the
    /// remaining stages are executed.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ImportCommand<C> {
    /// Execute `import` command
    pub async fn execute<N, E, F>(self, executor: F) -> eyre::Result<()>
    where
        N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>,
        E: BlockExecutorProvider,
        F: FnOnce(Arc<N::ChainSpec>) -> E,
    {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        if self.no_state {
            info!(target: "reth::cli", "Disabled stages requiring state");
        }

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
            "Chunking chain import"
        );

        let Environment { provider_factory, config, .. } = self.env.init::<N>(AccessRights::RW)?;

        let executor = executor(provider_factory.chain_spec());
        let consensus = Arc::new(EthBeaconConsensus::new(self.env.chain.clone()));
        info!(target: "reth::cli", "Consensus engine initialized");

        // open file
        let mut reader = ChunkedFileReader::new(&self.path, self.chunk_len).await?;

        let mut total_decoded_blocks = 0;
        let mut total_decoded_txns = 0;

        while let Some(file_client) = reader.next_chunk::<FileClient>().await? {
            // create a new FileClient from chunk read from file
            info!(target: "reth::cli",
                "Importing chain file chunk"
            );

            let tip = file_client.tip().ok_or(eyre::eyre!("file client has no tip"))?;
            info!(target: "reth::cli", "Chain file chunk read");

            total_decoded_blocks += file_client.headers_len();
            total_decoded_txns += file_client.total_transactions();

            let (mut pipeline, events) = build_import_pipeline(
                &config,
                provider_factory.clone(),
                &consensus,
                Arc::new(file_client),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
                self.no_state,
                executor.clone(),
            )?;

            // override the tip
            pipeline.set_tip(tip);
            debug!(target: "reth::cli", ?tip, "Tip manually set");

            let provider = provider_factory.provider()?;

            let latest_block_number =
                provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
            tokio::spawn(reth_node_events::node::handle_events(None, latest_block_number, events));

            // Run pipeline
            info!(target: "reth::cli", "Starting sync pipeline");
            tokio::select! {
                res = pipeline.run() => res?,
                _ = tokio::signal::ctrl_c() => {},
            }
        }

        let provider = provider_factory.provider()?;

        let total_imported_blocks = provider.tx_ref().entries::<tables::HeaderNumbers>()?;
        let total_imported_txns = provider.tx_ref().entries::<tables::TransactionHashNumbers>()?;

        if total_decoded_blocks != total_imported_blocks ||
            total_decoded_txns != total_imported_txns
        {
            error!(target: "reth::cli",
                total_decoded_blocks,
                total_imported_blocks,
                total_decoded_txns,
                total_imported_txns,
                "Chain was partially imported"
            );
        }

        info!(target: "reth::cli",
            total_imported_blocks,
            total_imported_txns,
            "Chain file imported"
        );

        Ok(())
    }
}

/// Builds import pipeline.
///
/// If configured to execute, all stages will run. Otherwise, only stages that don't require state
/// will run.
pub fn build_import_pipeline<N, C, E>(
    config: &Config,
    provider_factory: ProviderFactory<N>,
    consensus: &Arc<C>,
    file_client: Arc<FileClient>,
    static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    disable_exec: bool,
    executor: E,
) -> eyre::Result<(Pipeline<N>, impl Stream<Item = NodeEvent>)>
where
    N: ProviderNodeTypes,
    C: Consensus + 'static,
    E: BlockExecutorProvider,
{
    if !file_client.has_canonical_blocks() {
        eyre::bail!("unable to import non canonical blocks");
    }

    // Retrieve latest header found in the database.
    let last_block_number = provider_factory.last_block_number()?;
    let local_head = provider_factory
        .sealed_header(last_block_number)?
        .ok_or_else(|| ProviderError::HeaderNotFound(last_block_number.into()))?;

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
        .build(file_client.clone(), consensus.clone())
        .into_task();
    // TODO: The pipeline should correctly configure the downloader on its own.
    // Find the possibility to remove unnecessary pre-configuration.
    header_downloader.update_local_head(local_head);
    header_downloader.update_sync_target(SyncTarget::Tip(file_client.tip().unwrap()));

    let mut body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies)
        .build(file_client.clone(), consensus.clone(), provider_factory.clone())
        .into_task();
    // TODO: The pipeline should correctly configure the downloader on its own.
    // Find the possibility to remove unnecessary pre-configuration.
    body_downloader
        .set_download_range(file_client.min_block().unwrap()..=file_client.max_block().unwrap())
        .expect("failed to set download range");

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);

    let max_block = file_client.max_block().unwrap_or(0);

    let pipeline = Pipeline::<N>::builder()
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
                executor,
                config.stages.clone(),
                PruneModes::default(),
            )
            .builder()
            .disable_all_if(&StageId::STATE_REQUIRED, || disable_exec),
        )
        .build(provider_factory, static_file_producer);

    let events = pipeline.events().map(Into::into);

    Ok((pipeline, events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_cli::chainspec::{EthereumChainSpecParser, SUPPORTED_CHAINS};

    #[test]
    fn parse_common_import_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: ImportCommand<EthereumChainSpecParser> =
                ImportCommand::parse_from(["reth", "--chain", chain, "."]);
            assert_eq!(
                Ok(args.env.chain.chain),
                chain.parse::<reth_chainspec::Chain>(),
                "failed to parse chain {chain}"
            );
        }
    }
}
