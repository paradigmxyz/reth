//! Logic to export from database era1 block history
//! and injecting them into era1 files with `Era1Writer`.

use crate::calculate_td_by_number;
use alloy_consensus::{BlockHeader, Sealable, TxReceipt};
use alloy_primitives::{BlockNumber, U256};
use eyre::{eyre, Result};
use reth_era::{
    common::file_ops::{EraFileId, StreamWriter},
    e2s::types::IndexEntry,
    era1::{
        file::Era1Writer,
        types::{
            execution::{
                Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts,
                HeaderRecord, TotalDifficulty, MAX_BLOCKS_PER_ERA1,
            },
            group::{BlockIndex, Era1Id},
        },
    },
};
use reth_fs_util as fs;
use reth_primitives_traits::Block;
use reth_storage_api::{BlockNumReader, BlockReader, HeaderProvider};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{debug, info, warn};

const REPORT_INTERVAL_SECS: u64 = 10;
const ENTRY_HEADER_SIZE: usize = 8;
const VERSION_ENTRY_SIZE: usize = ENTRY_HEADER_SIZE;

/// Configuration to export block history
/// to era1 files
#[derive(Clone, Debug)]
pub struct ExportConfig {
    /// Directory to export era1 files to
    pub dir: PathBuf,
    /// First block to export
    pub first_block_number: BlockNumber,
    /// Last block to export
    pub last_block_number: BlockNumber,
    /// Number of blocks per era1 file
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

/// Fetches block history data from the provider
/// and prepares it for export to era1 files
/// for a given number of blocks then writes them to disk.
pub fn export<P>(provider: &P, config: &ExportConfig) -> Result<Vec<PathBuf>>
where
    P: BlockReader,
{
    config.validate()?;
    info!(
        "Exporting blockchain history from block {} to {} with this max of blocks per file of {}",
        config.first_block_number, config.last_block_number, config.max_blocks_per_file
    );

    // Determine the actual last block to export
    // best_block_number() might be outdated, so check actual block availability
    let last_block_number = determine_export_range(provider, config)?;

    info!(
        target: "era::history::export",
        first = config.first_block_number,
        last = last_block_number,
        max_blocks_per_file = config.max_blocks_per_file,
        "Preparing era1 export data"
    );

    if !config.dir.exists() {
        fs::create_dir_all(&config.dir)
            .map_err(|e| eyre!("Failed to create output directory: {}", e))?;
    }

    let start_time = Instant::now();
    let mut last_report_time = Instant::now();
    let report_interval = Duration::from_secs(REPORT_INTERVAL_SECS);

    let mut created_files = Vec::new();
    let mut total_blocks_processed = 0;

    let mut total_difficulty = if config.first_block_number > 0 {
        let prev_block_number = config.first_block_number - 1;
        calculate_td_by_number(provider, prev_block_number)?
    } else {
        U256::ZERO
    };

    // Process blocks in chunks according to `max_blocks_per_file`
    for start_block in
        (config.first_block_number..=last_block_number).step_by(config.max_blocks_per_file as usize)
    {
        let end_block = (start_block + config.max_blocks_per_file - 1).min(last_block_number);
        let block_count = (end_block - start_block + 1) as usize;

        info!(
            target: "era::history::export",
            "Processing blocks {start_block} to {end_block} ({block_count} blocks)"
        );

        let headers = provider.headers_range(start_block..=end_block)?;

        // Pre-compute accumulator from headers to determine filename
        let mut precompute_td = total_difficulty;
        let header_records: Vec<HeaderRecord> = headers
            .iter()
            .map(|h| {
                precompute_td += h.difficulty();
                HeaderRecord { block_hash: h.hash_slow(), total_difficulty: precompute_td }
            })
            .collect();
        let accumulator = Accumulator::from_header_records(&header_records)
            .map_err(|e| eyre!("Failed to compute accumulator: {e}"))?;
        let file_hash: [u8; 4] = accumulator.root[..4].try_into().unwrap();

        let era1_id =
            Era1Id::new(&config.network, start_block, block_count as u32).with_hash(file_hash);

        let era1_id = if config.max_blocks_per_file == MAX_BLOCKS_PER_ERA1 as u64 {
            era1_id
        } else {
            era1_id.with_era_count()
        };

        debug!("Final file name {}", era1_id.to_file_name());
        let file_path = config.dir.join(era1_id.to_file_name());
        let file = std::fs::File::create(&file_path)?;
        let mut writer = Era1Writer::new(file);
        writer.write_version()?;

        let mut offsets = Vec::<i64>::with_capacity(block_count);
        let mut position = VERSION_ENTRY_SIZE as i64;
        let mut blocks_written = 0;

        for (i, header) in headers.into_iter().enumerate() {
            let expected_block_number = start_block + i as u64;

            let (compressed_header, compressed_body, compressed_receipts) = compress_block_data(
                provider,
                header,
                expected_block_number,
                &mut total_difficulty,
            )?;

            let difficulty = TotalDifficulty::new(total_difficulty);

            let header_size = compressed_header.data.len() + ENTRY_HEADER_SIZE;
            let body_size = compressed_body.data.len() + ENTRY_HEADER_SIZE;
            let receipts_size = compressed_receipts.data.len() + ENTRY_HEADER_SIZE;
            let difficulty_size = 32 + ENTRY_HEADER_SIZE; // U256 is 32 + 8 bytes header overhead
            let total_size = (header_size + body_size + receipts_size + difficulty_size) as i64;

            let block_tuple = BlockTuple::new(
                compressed_header,
                compressed_body,
                compressed_receipts,
                difficulty,
            );

            offsets.push(position);
            position += total_size;

            writer.write_block(&block_tuple)?;
            blocks_written += 1;
            total_blocks_processed += 1;

            if last_report_time.elapsed() >= report_interval {
                info!(
                    target: "era::history::export",
                    "Export progress: block {expected_block_number}/{last_block_number} ({:.2}%) - elapsed: {:?}",
                    (total_blocks_processed as f64) /
                        ((last_block_number - config.first_block_number + 1) as f64) *
                        100.0,
                    start_time.elapsed()
                );
                last_report_time = Instant::now();
            }
        }
        if blocks_written > 0 {
            // Convert absolute offsets to relative (measured from block-index entry start)
            let accumulator_entry_size = (ENTRY_HEADER_SIZE + 32) as i64;
            let block_index_position = position + accumulator_entry_size;
            let relative_offsets: Vec<i64> =
                offsets.iter().map(|&abs| abs - block_index_position).collect();
            let block_index = BlockIndex::new(start_block, relative_offsets);

            writer.write_accumulator(&accumulator)?;
            writer.write_block_index(&block_index)?;
            writer.flush()?;

            info!(
                target: "era::history::export",
                "Wrote ERA1 file: {file_path:?} with {blocks_written} blocks"
            );
            created_files.push(file_path);
        }
    }

    info!(
        target: "era::history::export",
        "Successfully wrote {} ERA1 files in {:?}",
        created_files.len(),
        start_time.elapsed()
    );

    Ok(created_files)
}

// Determines the actual last block number that can be exported,
// Uses `headers_range` fallback when `best_block_number` is stale due to static file storage.
fn determine_export_range<P>(provider: &P, config: &ExportConfig) -> Result<BlockNumber>
where
    P: HeaderProvider + BlockNumReader,
{
    let best_block_number = provider.best_block_number()?;

    let last_block_number = if best_block_number < config.last_block_number {
        warn!(
            "Last block {} is beyond current head {}, setting last = head",
            config.last_block_number, best_block_number
        );

        // Check if more blocks are actually available beyond what `best_block_number()` reports
        if let Ok(headers) = provider.headers_range(best_block_number..=config.last_block_number) {
            if let Some(last_header) = headers.last() {
                let highest_block = last_header.number();
                info!("Found highest available block {} via headers_range", highest_block);
                highest_block
            } else {
                warn!("No headers found in range, using best_block_number {}", best_block_number);
                best_block_number
            }
        } else {
            warn!("headers_range failed, using best_block_number {}", best_block_number);
            best_block_number
        }
    } else {
        config.last_block_number
    };

    Ok(last_block_number)
}

// Compresses block data and returns compressed components with metadata
fn compress_block_data<P>(
    provider: &P,
    header: P::Header,
    expected_block_number: BlockNumber,
    total_difficulty: &mut U256,
) -> Result<(CompressedHeader, CompressedBody, CompressedReceipts)>
where
    P: BlockReader,
{
    let actual_block_number = header.number();

    if expected_block_number != actual_block_number {
        return Err(eyre!("Expected block {expected_block_number}, got {actual_block_number}"));
    }

    // CompressedBody must contain the block *body* (rlp(body)), not the full block (rlp(block)).
    let body = provider
        .block_by_number(actual_block_number)?
        .ok_or_else(|| eyre!("Block not found for block {}", actual_block_number))?
        .into_body();

    let receipts = provider
        .receipts_by_block(actual_block_number.into())?
        .ok_or_else(|| eyre!("Receipts not found for block {}", actual_block_number))?;

    *total_difficulty += header.difficulty();

    let compressed_header = CompressedHeader::from_header(&header)?;
    let compressed_body = CompressedBody::from_body(&body)?;
    let receipts_with_bloom: Vec<_> =
        receipts.iter().map(|r| TxReceipt::with_bloom_ref(r)).collect();
    let compressed_receipts = CompressedReceipts::from_encodable_list(&receipts_with_bloom)
        .map_err(|e| eyre!("Failed to compress receipts: {}", e))?;

    Ok((compressed_header, compressed_body, compressed_receipts))
}

#[cfg(test)]
mod tests {
    use crate::ExportConfig;
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
