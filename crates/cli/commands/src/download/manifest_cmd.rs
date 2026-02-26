use crate::download::manifest::generate_manifest;
use clap::Parser;
use eyre::Result;
use reth_static_file_types::DEFAULT_BLOCKS_PER_STATIC_FILE;
use std::{path::PathBuf, time::Instant};
use tracing::info;

/// Generate modular chunk archives and a snapshot manifest from a source datadir.
///
/// Archive naming convention:
///   - Chunked: `{component}-{start}-{end}.tar.zst` (e.g. `transactions-0-499999.tar.zst`)
#[derive(Debug, Parser)]
pub struct SnapshotManifestCommand {
    /// Source datadir containing static files.
    #[arg(long, short = 'd')]
    source_datadir: PathBuf,

    /// Optional base URL where archives will be hosted.
    #[arg(long)]
    base_url: Option<String>,

    /// Output directory where chunk archives and manifest.json are written.
    #[arg(long, short = 'o')]
    output_dir: PathBuf,

    /// Block number this snapshot was taken at.
    #[arg(long)]
    block: u64,

    /// Chain ID.
    #[arg(long, default_value = "1")]
    chain_id: u64,

    /// Blocks per archive file for chunked components.
    #[arg(long, default_value_t = DEFAULT_BLOCKS_PER_STATIC_FILE)]
    blocks_per_file: u64,
}

impl SnapshotManifestCommand {
    pub fn execute(self) -> Result<()> {
        info!(target: "reth::cli",
            dir = ?self.source_datadir,
            output = ?self.output_dir,
            "Packaging modular snapshot archives"
        );
        let start = Instant::now();
        let manifest = generate_manifest(
            &self.source_datadir,
            &self.output_dir,
            self.base_url.as_deref(),
            self.block,
            self.chain_id,
            self.blocks_per_file,
        )?;

        let num_components = manifest.components.len();
        let json = serde_json::to_string_pretty(&manifest)?;
        let output = self.output_dir.join("manifest.json");
        reth_fs_util::write(&output, &json)?;
        info!(target: "reth::cli",
            path = ?output,
            components = num_components,
            block = manifest.block,
            elapsed = ?start.elapsed(),
            "Manifest written"
        );

        Ok(())
    }
}
