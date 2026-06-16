//! Exports node block history from storage into ERA files.
//!
//! [`export`] is the format-agnostic driver: it resolves the export range, walks the requested
//! blocks in `max_blocks_per_file` chunks, and hands each chunk to an [`EraBlockWriter`]. A writer
//! turns one chunk of [`ExportBlock`]s into one on-disk file and owns every format-specific detail
//! (receipt encoding, accumulator, block index, record layout, file naming).
//!
//! [`Era1`](crate::Era1) writes `.era1` files.

mod era1;

use crate::calculate_td_by_number;
use alloy_consensus::{BlockHeader, Sealable};
use alloy_primitives::{BlockNumber, B256, U256};
use alloy_rlp::Encodable;
use eyre::{eyre, Result};
use reth_era::era1::types::execution::MAX_BLOCKS_PER_ERA1;
use reth_fs_util as fs;
use reth_primitives_traits::{Block, Receipt};
use reth_storage_api::{BlockNumReader, BlockReader, HeaderProvider, ReceiptProvider};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tracing::{info, warn};

/// Minimum delay between export progress log lines, so large exports report periodically without
/// flooding the logs.
const REPORT_INTERVAL_SECS: u64 = 10;

/// Configuration to export block history to ERA files.
///
/// Shared by every [`EraBlockWriter`]; the per-file block ceiling is the same `8192` for all
/// formats.
#[derive(Clone, Debug)]
pub struct ExportConfig {
    /// Directory to export ERA files to
    pub dir: PathBuf,
    /// First block to export
    pub first_block_number: BlockNumber,
    /// Last block to export
    pub last_block_number: BlockNumber,
    /// Number of blocks per ERA file.
    /// It can never be larger than `MAX_BLOCKS_PER_ERA1 = 8192`
    /// See also <`https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md`>
    pub max_blocks_per_file: u64,
    /// Network name.
    pub network: String,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::new(),
            first_block_number: 0,
            last_block_number: (MAX_BLOCKS_PER_ERA1 - 1) as u64,
            max_blocks_per_file: MAX_BLOCKS_PER_ERA1 as u64,
            network: "mainnet".to_string(),
        }
    }
}

impl ExportConfig {
    /// Validates the export configuration parameters
    pub fn validate(&self) -> Result<()> {
        if self.max_blocks_per_file > MAX_BLOCKS_PER_ERA1 as u64 {
            return Err(eyre!(
                "Max blocks per file ({}) exceeds ERA1 limit ({})",
                self.max_blocks_per_file,
                MAX_BLOCKS_PER_ERA1
            ));
        }

        if self.max_blocks_per_file == 0 {
            return Err(eyre!("Max blocks per file cannot be zero"));
        }

        Ok(())
    }
}

/// One block's data, gathered by [`export`] and handed to an [`EraBlockWriter`].
#[derive(Debug)]
pub struct ExportBlock<H, B, R> {
    /// Block header.
    pub header: H,
    /// Hash of `header`, feeding the file's accumulator.
    pub block_hash: B256,
    /// Block body.
    pub body: B,
    /// Block receipts in the provider's native, bloom-bearing form. A writer re-encodes them into
    /// its own form (`era1` keeps the bloom, `ere` stores the slim variant).
    pub receipts: Vec<R>,
    /// Total difficulty up to and including this block.
    pub total_difficulty: U256,
}

impl<H, B, R> ExportBlock<H, B, R> {
    /// The `(block_hash, total_difficulty)` pair that feeds an ERA file's header-record
    /// accumulator.
    ///
    /// The accumulator record type differs per format, so each writer wraps this pair in its own
    /// `HeaderRecord`.
    pub const fn header_record(&self) -> (B256, U256) {
        (self.block_hash, self.total_difficulty)
    }
}

/// Writes a chunk of consecutive blocks as a single ERA file.
///
/// One implementor exists per ERA format. A chunk is ordered, non-empty, and at most
/// [`ExportConfig::max_blocks_per_file`] blocks long.
pub trait EraBlockWriter {
    /// Writes `blocks` as a single ERA file in `dir`, returning the created file's path.
    ///
    /// `max_blocks_per_file` is the configured per-file ceiling; a writer compares it against its
    /// own format limit to decide whether the filename carries an era-count segment.
    fn write_file<H, B, R>(
        network: &str,
        max_blocks_per_file: u64,
        blocks: &[ExportBlock<H, B, R>],
        dir: &Path,
    ) -> Result<PathBuf>
    where
        H: BlockHeader + Encodable,
        B: Encodable,
        R: Receipt;
}

/// Per-format accumulator over a chunk's header records.
///
/// Each ERA format defines its own accumulator and header-record types in [`reth_era`], yet both
/// build the accumulator from the same `(block_hash, total_difficulty)` pairs. This trait is the
/// seam that keeps [`accumulator`] format-agnostic.
pub trait ChunkAccumulator: Sized {
    /// Builds the accumulator from the chunk's `(block_hash, total_difficulty)` header records.
    fn from_pairs(records: &[(B256, U256)]) -> Result<Self>;
}

/// Computes the chunk's accumulator from each block's `(block_hash, total_difficulty)` header
/// record, in the format `A`.
fn accumulator<A, H, B, R>(blocks: &[ExportBlock<H, B, R>]) -> Result<A>
where
    A: ChunkAccumulator,
{
    let records: Vec<(B256, U256)> = blocks.iter().map(ExportBlock::header_record).collect();
    A::from_pairs(&records)
}

/// A chunk of [`ExportBlock`]s sourced from provider `P`.
type Chunk<P> =
    Vec<ExportBlock<<P as HeaderProvider>::Header, BodyOf<P>, <P as ReceiptProvider>::Receipt>>;

/// The block body type produced by provider `P`.
type BodyOf<P> = <<P as BlockReader>::Block as Block>::Body;

/// Fetches block history from `provider` and writes it to ERA files in the `W` format, chunked by
/// [`ExportConfig::max_blocks_per_file`].
///
/// Returns the paths of the files that were created.
pub fn export<W, P>(provider: &P, config: &ExportConfig) -> Result<Vec<PathBuf>>
where
    W: EraBlockWriter,
    P: BlockReader,
    P::Header: BlockHeader + Sealable + Encodable,
    BodyOf<P>: Encodable,
    P::Receipt: Receipt,
{
    config.validate()?;

    // `best_block_number()` can be stale behind static files, so reconcile against what is actually
    // available.
    let last_block = determine_export_range(provider, config)?;

    info!(
        target: "era::history::export",
        first = config.first_block_number,
        last = last_block,
        max_blocks_per_file = config.max_blocks_per_file,
        "Preparing ERA export data"
    );

    if !config.dir.exists() {
        fs::create_dir_all(&config.dir)
            .map_err(|e| eyre!("Failed to create output directory: {}", e))?;
    }

    let mut progress = ExportProgress::new(last_block - config.first_block_number + 1);
    let mut total_difficulty = seed_total_difficulty(provider, config)?;
    let mut created_files = Vec::new();

    for start_block in
        (config.first_block_number..=last_block).step_by(config.max_blocks_per_file as usize)
    {
        let end_block = (start_block + config.max_blocks_per_file - 1).min(last_block);

        let blocks = gather_chunk(
            provider,
            start_block..=end_block,
            last_block,
            &mut total_difficulty,
            &mut progress,
        )?;
        if blocks.is_empty() {
            continue;
        }

        let file_path =
            W::write_file(&config.network, config.max_blocks_per_file, &blocks, &config.dir)?;

        info!(target: "era::history::export", "Wrote ERA file: {file_path:?} with {} blocks", blocks.len());
        created_files.push(file_path);
    }

    info!(
        target: "era::history::export",
        "Successfully wrote {} ERA files in {:?}",
        created_files.len(),
        progress.elapsed()
    );

    Ok(created_files)
}

/// The four-byte short hash an ERA file name carries, taken from its accumulator root.
fn short_hash(root: B256) -> [u8; 4] {
    root[..4].try_into().expect("root is 32 bytes")
}

/// Total difficulty up to the block preceding the export range, the starting point for the running
/// total threaded through every chunk.
fn seed_total_difficulty<P: BlockReader>(provider: &P, config: &ExportConfig) -> Result<U256> {
    if config.first_block_number > 0 {
        calculate_td_by_number(provider, config.first_block_number - 1)
    } else {
        Ok(U256::ZERO)
    }
}

/// Loads the headers, bodies and receipts for `range` into a [`Chunk`], advancing
/// `total_difficulty` by each header's difficulty.
fn gather_chunk<P>(
    provider: &P,
    range: std::ops::RangeInclusive<BlockNumber>,
    last_block: BlockNumber,
    total_difficulty: &mut U256,
    progress: &mut ExportProgress,
) -> Result<Chunk<P>>
where
    P: BlockReader,
    P::Header: BlockHeader + Sealable,
{
    let start_block = *range.start();
    let headers = provider.headers_range(range)?;
    let mut blocks = Vec::with_capacity(headers.len());

    for (i, header) in headers.into_iter().enumerate() {
        let expected = start_block + i as u64;
        let actual = header.number();
        if expected != actual {
            return Err(eyre!("Expected block {expected}, got {actual}"));
        }

        // `CompressedBody` holds rlp(body), not rlp(block), so take the body off the full block.
        let body = provider
            .block_by_number(actual)?
            .ok_or_else(|| eyre!("Block not found for block {actual}"))?
            .into_body();
        let receipts = provider
            .receipts_by_block(actual.into())?
            .ok_or_else(|| eyre!("Receipts not found for block {actual}"))?;

        let block_hash = header.hash_slow();
        *total_difficulty += header.difficulty();

        blocks.push(ExportBlock {
            header,
            block_hash,
            body,
            receipts,
            total_difficulty: *total_difficulty,
        });
        progress.record(actual, last_block);
    }

    Ok(blocks)
}

/// Determines the actual last block number that can be exported.
///
/// Uses a `headers_range` fallback when `best_block_number` is stale due to static file storage.
fn determine_export_range<P>(provider: &P, config: &ExportConfig) -> Result<BlockNumber>
where
    P: HeaderProvider + BlockNumReader,
{
    let best_block_number = provider.best_block_number()?;

    if best_block_number >= config.last_block_number {
        return Ok(config.last_block_number);
    }

    warn!(
        "Last block {} is beyond current head {}, setting last = head",
        config.last_block_number, best_block_number
    );

    // Check if more blocks are actually available beyond what `best_block_number()` reports.
    match provider.headers_range(best_block_number..=config.last_block_number) {
        Ok(headers) => match headers.last() {
            Some(last_header) => {
                let highest_block = last_header.number();
                info!("Found highest available block {} via headers_range", highest_block);
                Ok(highest_block)
            }
            None => {
                warn!("No headers found in range, using best_block_number {}", best_block_number);
                Ok(best_block_number)
            }
        },
        Err(_) => {
            warn!("headers_range failed, using best_block_number {}", best_block_number);
            Ok(best_block_number)
        }
    }
}

/// Throttled progress reporter for a running export.
struct ExportProgress {
    start: Instant,
    last_report: Instant,
    interval: Duration,
    total_blocks: u64,
    processed: u64,
}

impl ExportProgress {
    fn new(total_blocks: u64) -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last_report: now,
            interval: Duration::from_secs(REPORT_INTERVAL_SECS),
            total_blocks,
            processed: 0,
        }
    }

    /// Counts one processed block and logs progress at most once per [`REPORT_INTERVAL_SECS`].
    fn record(&mut self, current_block: BlockNumber, last_block: BlockNumber) {
        self.processed += 1;
        if self.last_report.elapsed() >= self.interval {
            info!(
                target: "era::history::export",
                "Export progress: block {current_block}/{last_block} ({:.2}%) - elapsed: {:?}",
                self.processed as f64 / self.total_blocks as f64 * 100.0,
                self.start.elapsed()
            );
            self.last_report = Instant::now();
        }
    }

    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::ExportConfig;
    use reth_era::era1::types::execution::MAX_BLOCKS_PER_ERA1;
    use tempfile::tempdir;

    #[test]
    fn test_export_config_validation() {
        let temp_dir = tempdir().unwrap();

        // Default config should pass
        let default_config = ExportConfig::default();
        assert!(default_config.validate().is_ok(), "Default config should be valid");

        // Exactly at the limit should pass
        let limit_config =
            ExportConfig { max_blocks_per_file: MAX_BLOCKS_PER_ERA1 as u64, ..Default::default() };
        assert!(limit_config.validate().is_ok(), "Config at ERA1 limit should pass validation");

        // Valid config should pass
        let valid_config = ExportConfig {
            dir: temp_dir.path().to_path_buf(),
            max_blocks_per_file: 1000,
            ..Default::default()
        };
        assert!(valid_config.validate().is_ok(), "Valid config should pass validation");

        // Zero blocks per file should fail
        let zero_blocks_config = ExportConfig {
            max_blocks_per_file: 0, // Invalid
            ..Default::default()
        };
        let result = zero_blocks_config.validate();
        assert!(result.is_err(), "Zero blocks per file should fail validation");
        assert!(result.unwrap_err().to_string().contains("cannot be zero"));

        // Exceeding era1 limit should fail
        let oversized_config = ExportConfig {
            max_blocks_per_file: MAX_BLOCKS_PER_ERA1 as u64 + 1, // Invalid
            ..Default::default()
        };
        let result = oversized_config.validate();
        assert!(result.is_err(), "Oversized blocks per file should fail validation");
        assert!(result.unwrap_err().to_string().contains("exceeds ERA1 limit"));
    }
}
