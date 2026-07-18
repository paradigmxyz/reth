//! Command that initializes the node by importing a chain from ERA files.
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use alloy_chains::{ChainKind, NamedChain};
use clap::{Args, Parser};
use eyre::eyre;
use reqwest::{Client, Url};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_era::{common::file_ops::EraFileType, ere::types::execution::SlimReceipt};
use reth_era_downloader::{read_dir, read_era_dir, EraClient, EraStream, EraStreamConfig};
use reth_era_utils as era;
use reth_etl::Collector;
use reth_fs_util as fs;
use reth_node_core::version::version_metadata;
use reth_primitives_traits::NodePrimitives;
use reth_provider::StaticFileProviderFactory;
use reth_static_file_types::StaticFileSegment;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

/// Syncs ERA encoded blocks from a local or remote source.
#[derive(Debug, Parser)]
pub struct ImportEraCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[clap(flatten)]
    import: ImportArgs,

    /// Stop the import after this block height has been reached.
    ///
    /// The file containing the block is imported up to and including this height, then the
    /// import ends. By default all available blocks are imported.
    #[arg(long, value_name = "TO_BLOCK", verbatim_doc_comment)]
    to_block: Option<u64>,

    /// Also import receipts, writing them to the `Receipts` static file segment.
    ///
    /// Off by default. Only `.era1` and `.ere` files may carry receipts (`.era1` always
    /// includes them; `.ere` receipts are optional per spec); passing this flag with plain
    /// `.era` files, or with `.ere` files that omit receipts, is an error.
    #[arg(long, verbatim_doc_comment)]
    with_receipts: bool,
}

#[derive(Debug, Args)]
#[group(required = false, multiple = false)]
pub struct ImportArgs {
    /// The path to a directory for import.
    ///
    /// The ERA1 files are read from the local directory parsing headers and bodies.
    #[arg(long, value_name = "IMPORT_ERA_PATH", verbatim_doc_comment)]
    path: Option<PathBuf>,

    /// The URL to a remote host where the ERA1 files are hosted.
    ///
    /// The ERA1 files are read from the remote host using HTTP GET requests parsing headers
    /// and bodies.
    #[arg(long, value_name = "IMPORT_ERA_URL", verbatim_doc_comment)]
    url: Option<Url>,
}

trait TryFromChain {
    fn try_to_url(&self) -> eyre::Result<Url>;
}

impl TryFromChain for ChainKind {
    fn try_to_url(&self) -> eyre::Result<Url> {
        Ok(match self {
            ChainKind::Named(NamedChain::Mainnet) => {
                Url::parse("https://era.ithaca.xyz/era1/index.html").expect("URL should be valid")
            }
            ChainKind::Named(NamedChain::Sepolia) => {
                Url::parse("https://era.ithaca.xyz/sepolia-era1/index.html")
                    .expect("URL should be valid")
            }
            chain => return Err(eyre!("No known host for ERA files on chain {chain:?}")),
        })
    }
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ImportEraCommand<C> {
    /// Execute `import-era` command
    pub async fn execute<N>(self, runtime: reth_tasks::Runtime) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
        <N::Primitives as NodePrimitives>::Receipt: From<SlimReceipt>,
    {
        info!(target: "reth::cli", "reth {} starting", version_metadata().short_version);

        let Environment { provider_factory, config, .. } =
            self.env.init::<N>(AccessRights::RW, runtime)?;

        let mut hash_collector = Collector::new(config.stages.etl.file_size, config.stages.etl.dir);

        let next_block = provider_factory
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .unwrap_or_default() +
            1;

        if let Some(path) = self.import.path {
            let era_type = EraFileType::from_dir(&path)?.ok_or_else(|| {
                eyre!(
                    "No ERA (.era), ERA1 (.era1) or ERE (.ere, .erae) files found in {}",
                    path.display()
                )
            })?;

            info!(target: "reth::cli", ?era_type, path = %path.display(), to_block = ?self.to_block, with_receipts = self.with_receipts, "Starting ERA import");

            check_receipts_supported(self.with_receipts, era_type)?;

            match era_type {
                EraFileType::Era => era::import::<era::Era, _, _, _, _, _, _>(
                    read_era_dir(path)?,
                    &provider_factory,
                    &mut hash_collector,
                    self.to_block,
                    self.with_receipts,
                )?,
                EraFileType::Ere => era::import::<era::Ere, _, _, _, _, _, _>(
                    read_dir(path, next_block)?,
                    &provider_factory,
                    &mut hash_collector,
                    self.to_block,
                    self.with_receipts,
                )?,
                EraFileType::Era1 => era::import::<era::Era1, _, _, _, _, _, _>(
                    read_dir(path, next_block)?,
                    &provider_factory,
                    &mut hash_collector,
                    self.to_block,
                    self.with_receipts,
                )?,
            };
        } else {
            let url = match self.import.url {
                Some(url) => url,
                None => self.env.chain.chain().kind().try_to_url()?,
            };
            let era_type = EraFileType::from_url(url.as_str());

            info!(target: "reth::cli", ?era_type, %url, to_block = ?self.to_block, with_receipts = self.with_receipts, "Starting ERA import");

            check_receipts_supported(self.with_receipts, era_type)?;

            let folder =
                self.env.datadir.resolve_datadir(self.env.chain.chain()).data_dir().join("era");

            fs::create_dir_all(&folder)?;

            let mut config = EraStreamConfig::default();
            // `start_from` maps a block number to a file index as `block / BLOCKS_PER_FILE`, valid
            // only for execution-layer files (era1/ere). Consensus `.era` files are slot-indexed,
            // so stream from 0 and let the pipeline skip already-imported blocks.
            if !matches!(era_type, EraFileType::Era) {
                config = config.start_from(next_block);
            }
            let client = EraClient::new(Client::new(), url, folder).with_era_type(era_type);
            let stream = EraStream::new(client, config);

            match era_type {
                EraFileType::Ere => era::import::<era::Ere, _, _, _, _, _, _>(
                    stream,
                    &provider_factory,
                    &mut hash_collector,
                    self.to_block,
                    self.with_receipts,
                )?,
                EraFileType::Era1 => era::import::<era::Era1, _, _, _, _, _, _>(
                    stream,
                    &provider_factory,
                    &mut hash_collector,
                    self.to_block,
                    self.with_receipts,
                )?,
                EraFileType::Era => era::import::<era::Era, _, _, _, _, _, _>(
                    stream,
                    &provider_factory,
                    &mut hash_collector,
                    self.to_block,
                    self.with_receipts,
                )?,
            };
        }

        Ok(())
    }
}

/// Errors if `--with-receipts` was passed for the consensus `.era` format, which carries no
/// receipt data at all.
fn check_receipts_supported(with_receipts: bool, era_type: EraFileType) -> eyre::Result<()> {
    if with_receipts && era_type == EraFileType::Era {
        return Err(eyre!(
            "--with-receipts is not supported for `.era` files: they contain no receipt data. \
             Use `.era1` or `.ere` files instead."
        ));
    }
    Ok(())
}

impl<C: ChainSpecParser> ImportEraCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}
