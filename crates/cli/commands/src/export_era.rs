//! Command exporting block data to convert them to ERA1 files.

use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{Args, Parser};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_era::execution_types::MAX_BLOCKS_PER_ERA1;
use reth_era_utils as era1;
use reth_provider::DatabaseProviderFactory;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

// Default folder name for era1 export files
const ERA1_EXPORT_FOLDER_NAME: &str = "era1-export";

#[derive(Debug, Parser)]
pub struct ExportEraCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[clap(flatten)]
    export: ExportArgs,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Optional first block number to export from the db.
    /// It is by default 0.
    #[arg(long, value_name = "first-block-number", verbatim_doc_comment)]
    first_block_number: Option<u64>,
    /// Optional last block number to export from the db.
    /// It is by default 8191.
    #[arg(long, value_name = "last-block-number", verbatim_doc_comment)]
    last_block_number: Option<u64>,
    /// The maximum number of blocks per file, it can help you to decrease the size of the files.
    /// Must be less than or equal to 8192.
    #[arg(long, value_name = "max-blocks-per-file", verbatim_doc_comment)]
    max_blocks_per_file: Option<u64>,
    /// The directory path where to export era1 files.
    /// The block data are read from the database.
    #[arg(long, value_name = "EXPORT_ERA1_PATH", verbatim_doc_comment)]
    path: Option<PathBuf>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ExportEraCommand<C> {
    /// Execute `export-era` command
    pub async fn execute<N>(self) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO)?;

        // Either specified path or default to `<data-dir>/<chain>/era1-export/`
        let data_dir = match &self.export.path {
            Some(path) => path.clone(),
            None => self
                .env
                .datadir
                .resolve_datadir(self.env.chain.chain())
                .data_dir()
                .join(ERA1_EXPORT_FOLDER_NAME),
        };

        let export_config = era1::ExportConfig {
            network: self.env.chain.chain().to_string(),
            first_block_number: self.export.first_block_number.unwrap_or(0),
            last_block_number: self
                .export
                .last_block_number
                .unwrap_or(MAX_BLOCKS_PER_ERA1 as u64 - 1),
            max_blocks_per_file: self
                .export
                .max_blocks_per_file
                .unwrap_or(MAX_BLOCKS_PER_ERA1 as u64),
            dir: data_dir,
        };

        export_config.validate()?;

        info!(
            target: "reth::cli",
            "Starting ERA1 block export: blocks {}-{} to {}",
            export_config.first_block_number,
            export_config.last_block_number,
            export_config.dir.display()
        );

        // Only read access is needed for the database provider
        let provider = provider_factory.database_provider_ro()?;

        let exported_files = era1::export(&provider, &export_config)?;

        info!(
            target: "reth::cli",
            "Successfully exported {} ERA1 files to {}",
            exported_files.len(),
            export_config.dir.display()
        );

        Ok(())
    }
}

impl<C: ChainSpecParser> ExportEraCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}
