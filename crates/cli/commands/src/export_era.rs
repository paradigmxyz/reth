//! Command that exports block history from the database into ERA files.

use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{Args, Parser};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_era::{common::file_ops::EraFileType, era1::types::execution::MAX_BLOCKS_PER_ERA1};
use reth_era_utils as era;
use reth_provider::DatabaseProviderFactory;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

#[derive(Debug, Parser)]
pub struct ExportEraCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[clap(flatten)]
    export: ExportArgs,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The ERA file format to export: `era1` writes `.era1` files, `ere` writes `.ere` files.
    #[arg(long, value_parser = parse_export_file_type, default_value = "era1", verbatim_doc_comment)]
    file_type: EraFileType,
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
    /// The directory path where to export the ERA files.
    /// The block data are read from the database.
    #[arg(long, value_name = "EXPORT_PATH", verbatim_doc_comment)]
    path: Option<PathBuf>,
}

/// Parses the `--file-type` value, accepting only the exportable execution-layer formats.
///
/// Consensus-layer `.era` files cannot be produced from the execution database, so
/// [`EraFileType::Era`] is rejected.
fn parse_export_file_type(value: &str) -> Result<EraFileType, String> {
    match value {
        "era1" => Ok(EraFileType::Era1),
        "ere" => Ok(EraFileType::Ere),
        other => Err(format!("invalid --file-type '{other}', expected 'era1' or 'ere'")),
    }
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ExportEraCommand<C> {
    /// Execute `export-era` command
    pub async fn execute<N>(self, runtime: reth_tasks::Runtime) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO, runtime)?;

        let file_type = self.export.file_type;
        // `.era1` / `.ere` -> `era1` / `ere`, used for log lines and the default directory name.
        let format = file_type.extension().trim_start_matches('.');

        // Either the specified path or default to `<data-dir>/<chain>/<format>-export/`.
        let data_dir = match &self.export.path {
            Some(path) => path.clone(),
            None => self
                .env
                .datadir
                .resolve_datadir(self.env.chain.chain())
                .data_dir()
                .join(format!("{format}-export")),
        };

        let export_config = era::ExportConfig {
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
            "Starting {format} block export: blocks {}-{} to {}",
            export_config.first_block_number,
            export_config.last_block_number,
            export_config.dir.display()
        );

        // Only read access is needed for the database provider.
        let provider = provider_factory.database_provider_ro()?;

        let exported_files = match file_type {
            EraFileType::Era1 => era::export::<era::Era1, _>(&provider, &export_config)?,
            EraFileType::Ere => era::export::<era::Ere, _>(&provider, &export_config)?,
            // Consensus-layer `.era` files cannot be produced from the execution database.
            EraFileType::Era => {
                eyre::bail!("consensus-layer .era files are not yet supported for export")
            }
        };

        info!(
            target: "reth::cli",
            "Successfully exported {} {format} files to {}",
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
