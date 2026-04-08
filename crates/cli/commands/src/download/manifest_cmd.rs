use crate::download::manifest::generate_manifest;
use clap::Parser;
use eyre::{Result, WrapErr};
use reth_db::{mdbx::DatabaseArguments, open_db_read_only, tables, Database};
use reth_db_api::transaction::DbTx;
use reth_primitives_traits::FastInstant as Instant;
use reth_stages_types::StageId;
use reth_static_file_types::DEFAULT_BLOCKS_PER_STATIC_FILE;
use std::path::PathBuf;
use tracing::{info, warn};

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
    ///
    /// If omitted, this is inferred from the source datadir's `Finish` stage checkpoint.
    #[arg(long)]
    block: Option<u64>,

    /// Chain ID.
    #[arg(long, default_value = "1")]
    chain_id: u64,

    /// Blocks per archive file for chunked components.
    ///
    /// If omitted, this is inferred from header static file ranges in the source datadir.
    #[arg(long)]
    blocks_per_file: Option<u64>,
}

impl SnapshotManifestCommand {
    pub fn execute(self) -> Result<()> {
        let block = match self.block {
            Some(block) => block,
            None => infer_snapshot_block(&self.source_datadir)?,
        };
        let blocks_per_file = match self.blocks_per_file {
            Some(blocks_per_file) => blocks_per_file,
            None => infer_blocks_per_file(&self.source_datadir)?,
        };

        info!(target: "reth::cli",
            dir = ?self.source_datadir,
            output = ?self.output_dir,
            block,
            blocks_per_file,
            "Packaging modular snapshot archives"
        );
        let start = Instant::now();
        let manifest = generate_manifest(
            &self.source_datadir,
            &self.output_dir,
            self.base_url.as_deref(),
            block,
            self.chain_id,
            blocks_per_file,
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

fn infer_snapshot_block(source_datadir: &std::path::Path) -> Result<u64> {
    if let Ok(block) = infer_snapshot_block_from_db(source_datadir) {
        return Ok(block);
    }

    let block = infer_snapshot_block_from_headers(source_datadir)?;
    warn!(
        target: "reth::cli",
        block,
        "Could not read Finish stage checkpoint from source DB, using header static-file tip"
    );
    Ok(block)
}

fn infer_snapshot_block_from_db(source_datadir: &std::path::Path) -> Result<u64> {
    let candidates = [source_datadir.join("db"), source_datadir.to_path_buf()];

    for db_path in candidates {
        if !db_path.exists() {
            continue;
        }

        let db = match open_db_read_only(&db_path, DatabaseArguments::default()) {
            Ok(db) => db,
            Err(_) => continue,
        };

        let tx = db.tx()?;
        if let Some(checkpoint) = tx.get::<tables::StageCheckpoints>(StageId::Finish.to_string())? {
            return Ok(checkpoint.block_number);
        }
    }

    eyre::bail!(
        "Could not infer --block from source DB (Finish checkpoint missing); pass --block manually"
    )
}

fn infer_snapshot_block_from_headers(source_datadir: &std::path::Path) -> Result<u64> {
    let max_end = header_ranges(source_datadir)?
        .into_iter()
        .map(|(_, end)| end)
        .max()
        .ok_or_else(|| eyre::eyre!("No header static files found to infer --block"))?;
    Ok(max_end)
}

fn infer_blocks_per_file(source_datadir: &std::path::Path) -> Result<u64> {
    let mut inferred = None;
    for (start, end) in header_ranges(source_datadir)? {
        let span = end.saturating_sub(start).saturating_add(1);
        if span == 0 {
            continue;
        }

        if let Some(existing) = inferred {
            if existing != span {
                eyre::bail!(
                    "Inconsistent header static file ranges; pass --blocks-per-file manually"
                );
            }
        } else {
            inferred = Some(span);
        }
    }

    inferred.ok_or_else(|| {
        eyre::eyre!(
            "Could not infer --blocks-per-file from header static files; pass it manually (default is {DEFAULT_BLOCKS_PER_STATIC_FILE})"
        )
    })
}

fn header_ranges(source_datadir: &std::path::Path) -> Result<Vec<(u64, u64)>> {
    let static_files_dir = source_datadir.join("static_files");
    let static_files_dir =
        if static_files_dir.exists() { static_files_dir } else { source_datadir.to_path_buf() };

    let entries = std::fs::read_dir(&static_files_dir).wrap_err_with(|| {
        format!("Failed to read static files directory: {}", static_files_dir.display())
    })?;

    let mut ranges = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if let Some(range) = parse_headers_range(&file_name) {
            ranges.push(range);
        }
    }

    Ok(ranges)
}

fn parse_headers_range(file_name: &str) -> Option<(u64, u64)> {
    let remainder = file_name.strip_prefix("static_file_headers_")?;
    let (start, end_with_suffix) = remainder.split_once('_')?;

    let start = start.parse::<u64>().ok()?;
    let end_digits: String = end_with_suffix.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    let end = end_digits.parse::<u64>().ok()?;

    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_headers_range_works_with_suffixes() {
        assert_eq!(parse_headers_range("static_file_headers_0_499999"), Some((0, 499_999)));
        assert_eq!(
            parse_headers_range("static_file_headers_500000_999999.jar"),
            Some((500_000, 999_999))
        );
        assert_eq!(parse_headers_range("static_file_transactions_0_499999"), None);
    }

    #[test]
    fn infer_blocks_per_file_from_header_ranges() {
        let dir = tempdir().unwrap();
        let sf = dir.path().join("static_files");
        std::fs::create_dir_all(&sf).unwrap();
        std::fs::write(sf.join("static_file_headers_0_499999"), []).unwrap();
        std::fs::write(sf.join("static_file_headers_500000_999999.jar"), []).unwrap();

        assert_eq!(infer_blocks_per_file(dir.path()).unwrap(), 500_000);
    }

    #[test]
    fn infer_snapshot_block_from_headers_uses_max_end() {
        let dir = tempdir().unwrap();
        let sf = dir.path().join("static_files");
        std::fs::create_dir_all(&sf).unwrap();
        std::fs::write(sf.join("static_file_headers_0_499999"), []).unwrap();
        std::fs::write(sf.join("static_file_headers_500000_999999"), []).unwrap();

        assert_eq!(infer_snapshot_block_from_headers(dir.path()).unwrap(), 999_999);
    }
}
