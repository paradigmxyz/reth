use crate::download::manifest::generate_manifest;
use clap::Parser;
use eyre::Result;
use reth_static_file_types::DEFAULT_BLOCKS_PER_STATIC_FILE;
use std::{path::PathBuf, time::Instant};
use tracing::info;

/// Generate a snapshot manifest from local archive files.
///
/// Scans a directory for archive files matching the naming convention, computes sizes and
/// SHA-256 checksums, and writes a `manifest.json` file.
///
/// Archive naming convention:
///   - State: `state.tar.zst`
///   - Chunked: `{component}-{start}-{end}.tar.zst` (e.g. `transactions-0-499999.tar.zst`)
#[derive(Debug, Parser)]
pub struct SnapshotManifestCommand {
    /// Directory containing the archive files.
    #[arg(long, short = 'd')]
    archive_dir: PathBuf,

    /// Base URL where archives will be hosted. Used as the `base_url` field in the manifest.
    #[arg(long)]
    base_url: String,

    /// Block number this snapshot was taken at.
    #[arg(long)]
    block: u64,

    /// Chain ID.
    #[arg(long, default_value = "1")]
    chain_id: u64,

    /// Blocks per archive file for chunked components.
    #[arg(long, default_value_t = DEFAULT_BLOCKS_PER_STATIC_FILE)]
    blocks_per_file: u64,

    /// Output path for the manifest file.
    #[arg(long, short = 'o', default_value = "manifest.json")]
    output: PathBuf,
}

impl SnapshotManifestCommand {
    pub fn execute(self) -> Result<()> {
        info!(target: "reth::cli",
            dir = ?self.archive_dir,
            "Scanning archives and computing checksums (this may take a while for large files)"
        );
        let start = Instant::now();
        let manifest = generate_manifest(
            &self.archive_dir,
            &self.base_url,
            self.block,
            self.chain_id,
            self.blocks_per_file,
        )?;

        let num_components = manifest.components.len();
        let json = serde_json::to_string_pretty(&manifest)?;

        let output = if self.output.is_relative() {
            self.archive_dir.join(&self.output)
        } else {
            self.output
        };

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
