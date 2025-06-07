//! Logic to export from database era1 block history
//! and injecting them into era1 files with `Era1Writer`.

use alloy_consensus::{BlockBody, BlockHeader, Header};
use alloy_primitives::{BlockNumber, B256, U256};
use eyre::{eyre, Result};
use reth_era::{
    era1_file::Era1Writer,
    era1_types::{BlockIndex, Era1Id},
    execution_types::{
        Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts,
        TotalDifficulty,
    },
};
use reth_ethereum_primitives::TransactionSigned;
use reth_fs_util as fs;
use reth_primitives_traits::{Block, FullBlockHeader, SignedTransaction};
use reth_storage_api::{BlockReader, BlockWriter, DBProvider, HeaderProvider, ReceiptProvider};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{info, warn};

const REPORT_INTERVAL_SECS: u64 = 10;
const ENTRY_HEADER_SIZE: usize = 8;
const VERSION_ENTRY_SIZE: usize = ENTRY_HEADER_SIZE;

/// Configuration to export block history
/// to era1 files
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ExportConfig {
    /// Directory to export era1 files to
    pub dir: PathBuf,
    /// First block to export
    pub first_block_number: BlockNumber,
    /// Last block to export
    pub last_block_number: BlockNumber,
    /// Number of blocks per era1 file
    pub step: u64,
    /// Network name
    pub network: String,
}

/// Fetches block history data from the provider
/// and prepares it for export to era1 files
/// for a given number of blocks
/// then writes them to disk.
pub fn export<P, B>(provider: &P, config: &ExportConfig) -> Result<Vec<PathBuf>>
where
    P: DBProvider
        + BlockReader<Block = B>
        + ReceiptProvider
        + HeaderProvider
        + BlockWriter<Block = B>,
    B: Block + Into<BlockBody<P::Transaction, Header>>,
    P::Header: Into<Header> + FullBlockHeader,
    <P as ReceiptProvider>::Receipt: alloy_rlp::Encodable,
    P::Transaction: SignedTransaction + From<TransactionSigned>,
{
    info!(
        "Exporting blockchain history from block {} to {} in steps of {}",
        config.first_block_number, config.last_block_number, config.step
    );

    let head_block_number = provider.best_block_number()?;

    let last_block_number = if head_block_number < config.last_block_number {
        warn!(
            "Last block {} is beyond current head {}, setting last = head",
            config.last_block_number, head_block_number
        );
        head_block_number
    } else {
        config.last_block_number
    };

    info!(
        target: "era::history::export",
        first = config.first_block_number,
        last = config.last_block_number,
        step = config.step,
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
        provider
            .header_td_by_number(prev_block_number)?
            .ok_or_else(|| eyre!("Total difficulty not found for block {prev_block_number}"))?
    } else {
        U256::ZERO
    };

    // Process blocks in chunks according to step size
    for start_block in (config.first_block_number..=last_block_number).step_by(config.step as usize)
    {
        let end_block = (start_block + config.step - 1).min(last_block_number);
        let block_count = (end_block - start_block + 1) as usize;

        info!(
            target: "era::history::export",
            "Processing blocks {start_block} to {end_block} ({block_count} blocks)"
        );

        let headers = provider.headers_range(start_block..=end_block)?;

        let era1_id = Era1Id::new(&config.network, start_block, block_count as u32);
        let file_path = config.dir.join(era1_id.to_file_name());
        let file = std::fs::File::create(&file_path)?;
        let mut writer = Era1Writer::new(file);
        writer.write_version()?;

        let mut offsets = Vec::with_capacity(block_count);
        let mut position = VERSION_ENTRY_SIZE as i64;
        let mut blocks_written = 0;
        let mut final_header_data = Vec::new();

        for (i, header) in headers.into_iter().enumerate() {
            let expected_block_number = start_block + i as u64;
            let actual_block_number = header.number();

            // Validate block number
            if expected_block_number != actual_block_number {
                return Err(eyre!(
                    "Expected header for block {expected_block_number}, got {actual_block_number}"
                ));
            }

            let body = provider
                .block_by_number(actual_block_number)?
                .ok_or_else(|| eyre!("Block body not found for block {}", actual_block_number))?;

            let receipts = provider
                .receipts_by_block(actual_block_number.into())?
                .ok_or_else(|| eyre!("Receipts not found for block {}", actual_block_number))?;

            total_difficulty += header.difficulty();

            let compressed_header = CompressedHeader::from_header(&header.into())?;
            let compressed_body = CompressedBody::from_body(&body.into())?;
            let compressed_receipts = CompressedReceipts::from_encodable_list(&receipts)
                .map_err(|e| eyre!("Failed to compress receipts: {}", e))?;

            // Save last block's header data for accumulator
            if actual_block_number == end_block {
                final_header_data = compressed_header.data.clone();
            }

            let difficulty = TotalDifficulty::new(total_difficulty);

            let block_tuple = BlockTuple::new(
                compressed_header.clone(),
                compressed_body.clone(),
                compressed_receipts.clone(),
                difficulty,
            );

            let header_size = compressed_header.data.len() + ENTRY_HEADER_SIZE;
            let body_size = compressed_body.data.len() + ENTRY_HEADER_SIZE;
            let receipts_size = compressed_receipts.data.len() + ENTRY_HEADER_SIZE;
            let difficulty_size = 32 + ENTRY_HEADER_SIZE; // U256 is 32 + 8 bytes header overhead
            let total_size = header_size + body_size + receipts_size + difficulty_size;
            offsets.push(position);
            position += total_size as i64;

            writer.write_block(&block_tuple)?;
            blocks_written += 1;
            total_blocks_processed += 1;

            if last_report_time.elapsed() >= report_interval {
                info!(
                    target: "era::history::export",
                    "Export progress: block {actual_block_number}/{last_block_number} ({:.2}%) - elapsed: {:?}",
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
            let accumulator = Accumulator::new(accumulator_hash);
            let block_index = BlockIndex::new(start_block, offsets);

            writer.write_accumulator(&accumulator)?;
            writer.write_block_index(&block_index)?;
            writer.flush()?;
            created_files.push(file_path.clone());

            info!(
                target: "era::history::export",
                "Wrote ERA1 file: {file_path:?} with {blocks_written} blocks"
            );
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
