//! Logic to export from database erae block history
//! and injecting them into erae files with `EraEWriter`.

use crate::calculate_td_by_number;
use alloy_consensus::{BlockHeader, Sealable, TxReceipt};
use alloy_primitives::{BlockNumber, U256};
use eyre::{eyre, Result};
use reth_era::{
    common::file_ops::{EraFileId, StreamWriter},
    erae::{
        file::EraEWriter,
        types::{
            execution::{
                Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedSlimReceipts,
                TotalDifficulty, MAX_BLOCKS_PER_ERAE,
            },
            group::{BlockIndex, EraEId},
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
/// to erae files
#[derive(Clone, Debug)]
pub struct ExportConfig {
    /// Directory to export erae files to
    pub dir: PathBuf,
    /// First block to export
    pub first_block_number: BlockNumber,
    /// Last block to export
    pub last_block_number: BlockNumber,
    /// Number of blocks per erae file
    /// It can never be larger than `MAX_BLOCKS_PER_ERAE = 8192`
    /// See also <`https://github.com/eth-clients/e2store-format-specs/blob/main/formats/erae.md`>
    pub max_blocks_per_file: u64,
    /// Network name.
    pub network: String,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::new(),
            first_block_number: 0,
            last_block_number: (MAX_BLOCKS_PER_ERAE - 1) as u64,
            max_blocks_per_file: MAX_BLOCKS_PER_ERAE as u64,
            network: "mainnet".to_string(),
        }
    }
}

impl ExportConfig {
    /// Validates the export configuration parameters
    pub fn validate(&self) -> Result<()> {
        if self.max_blocks_per_file > MAX_BLOCKS_PER_ERAE as u64 {
            return Err(eyre!(
                "Max blocks per file ({}) exceeds ERAE limit ({})",
                self.max_blocks_per_file,
                MAX_BLOCKS_PER_ERAE
            ));
        }

        if self.max_blocks_per_file == 0 {
            return Err(eyre!("Max blocks per file cannot be zero"));
        }

        Ok(())
    }
}

/// Fetches block history data from the provider
/// and prepares it for export to erae files
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
        "Preparing erae export data"
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

        let erae_id = EraEId::new(&config.network, start_block, block_count as u32)
            .with_hash(historical_root);

        let erae_id = if config.max_blocks_per_file == MAX_BLOCKS_PER_ERAE as u64 {
            erae_id
        } else {
            erae_id.with_era_count()
        };

        debug!("Final file name {}", erae_id.to_file_name());
        let file_path = config.dir.join(erae_id.to_file_name());
        let file = std::fs::File::create(&file_path)?;
        let mut writer = EraEWriter::new(file);
        writer.write_version()?;

        let component_count = 4u64; // header + body + receipts + td
        let mut offsets = Vec::<i64>::with_capacity(block_count * component_count as usize);
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

            let header_size = (compressed_header.data.len() + ENTRY_HEADER_SIZE) as i64;
            let body_size = (compressed_body.data.len() + ENTRY_HEADER_SIZE) as i64;
            let receipts_size = (compressed_receipts.data.len() + ENTRY_HEADER_SIZE) as i64;
            let difficulty_size = (32 + ENTRY_HEADER_SIZE) as i64;

            let block_tuple = BlockTuple::new(
                compressed_header,
                compressed_body,
                compressed_receipts,
                difficulty,
            );

            // Track per-component offsets
            offsets.push(position); // header
            offsets.push(position + header_size); // body
            offsets.push(position + header_size + body_size); // receipts
            offsets.push(position + header_size + body_size + receipts_size); // td
            position += header_size + body_size + receipts_size + difficulty_size;

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
            let accumulator_hash =
                B256::from_slice(&final_header_data[0..32.min(final_header_data.len())]);
            let accumulator = Some(Accumulator::new(accumulator_hash));
            let block_index = BlockIndex::new(start_block, component_count, offsets);

            if let Some(ref acc) = accumulator {
                writer.write_accumulator(acc)?;
            }
            writer.write_block_index(&block_index)?;
            writer.flush()?;

            info!(
                target: "era::history::export",
                "Wrote ERAE file: {file_path:?} with {blocks_written} blocks"
            );
            created_files.push(file_path);
        }
    }

    info!(
        target: "era::history::export",
        "Successfully wrote {} ERAE files in {:?}",
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
) -> Result<(CompressedHeader, CompressedBody, CompressedSlimReceipts)>
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
    let compressed_receipts = CompressedSlimReceipts::from_encodable_list(&receipts)
        .map_err(|e| eyre!("Failed to compress receipts: {}", e))?;

    Ok((compressed_header, compressed_body, compressed_receipts))
}

#[cfg(test)]
mod tests {
    use crate::ExportConfig;
    use reth_era::erae::types::execution::MAX_BLOCKS_PER_ERAE;
    use tempfile::tempdir;

    #[test]
    fn test_export_config_validation() {
        let temp_dir = tempdir().unwrap();

        // Default config should pass
        let default_config = ExportConfig::default();
        assert!(default_config.validate().is_ok(), "Default config should be valid");

        // Exactly at the limit should pass
        let limit_config =
            ExportConfig { max_blocks_per_file: MAX_BLOCKS_PER_ERAE as u64, ..Default::default() };
        assert!(limit_config.validate().is_ok(), "Config at ERAE limit should pass validation");

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

        // Exceeding erae limit should fail
        let oversized_config = ExportConfig {
            max_blocks_per_file: MAX_BLOCKS_PER_ERAE as u64 + 1, // Invalid
            ..Default::default()
        };
        let result = oversized_config.validate();
        assert!(result.is_err(), "Oversized blocks per file should fail validation");
        assert!(result.unwrap_err().to_string().contains("exceeds ERAE limit"));
    }
}
