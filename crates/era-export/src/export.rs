//! Logic to export from database era1 block history
//! and injecting them into era1 files with `Era1Writer`.

use alloy_consensus::{BlockBody, BlockHeader, Header};
use alloy_primitives::{BlockNumber, U256};
use eyre::{eyre, Result};
use reth_era::execution_types::{
    BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts, TotalDifficulty,
};
use reth_primitives_traits::{FullBlockBody, FullBlockHeader, NodePrimitives};
use reth_storage_api::{
    BlockReader, DBProvider, HeaderProvider, NodePrimitivesProvider, ReceiptProvider,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{info, warn};

/// Configuration to export block history
/// to era1 files
#[allow(dead_code)]
pub(crate) struct ExportConfig {
    /// Directory to export era1 files to
    pub dir: PathBuf,
    /// First block to export
    pub first_block_number: BlockNumber,
    /// Last block to export
    pub last_block_number: BlockNumber,
    /// Number of blocks per era1 file
    /// TODO: check if we can determine it
    /// from the volume to export instead
    pub step: u64,
    /// Network name
    pub network: String,
}

/// Era export data
/// prepared to create (multiple?) era1 file
#[allow(dead_code)]
pub(crate) struct EraExportData {
    /// Block tuples containing header, body, receipts, and total difficulty
    pub block_tuples: Vec<BlockTuple>,
    /// Starting block number
    pub start_block: BlockNumber,
    /// Network name
    pub network: String,
    /// Block position offsets within the file
    pub offsets: Vec<i64>,
}

/// Fetches block history data from the provider
/// and prepares it for export to era1 files.
/// for a given number of blocks
/// TODO: add the write logic to create the era1 files
/// directly from this function
/// to avoid holding the data in memory
#[allow(dead_code)]
pub(crate) fn fetch_block_history_data<P, BH, BB>(
    provider: &P,
    config: &ExportConfig,
) -> Result<Vec<EraExportData>>
where
    P: DBProvider + HeaderProvider + BlockReader + ReceiptProvider + NodePrimitivesProvider,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
    <P as HeaderProvider>::Header: Into<Header> + FullBlockHeader,
    <P as BlockReader>::Block: FullBlockBody
        + Into<
            BlockBody<
                <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
                Header,
            >,
        >,
    <P as ReceiptProvider>::Receipt: alloy_rlp::Encodable,
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
        "Preparing ERA export data"
    );

    let start_time = Instant::now();
    let mut last_report_time = Instant::now();
    let report_interval = Duration::from_secs(8);

    // Prepare export data in chunks based on step size
    let mut export_data: Vec<EraExportData> = Vec::new();
    let mut total_blocks_processed = 0;

    let mut total_difficulty = if config.first_block_number > 0 {
        let prev_block_number = config.first_block_number - 1;
        provider
            .header_td_by_number(prev_block_number)?
            .ok_or_else(|| eyre!("Total difficulty not found for block {}", prev_block_number))?
    } else {
        U256::ZERO
    };

    // Process blocks in chunks according to step size
    for start_block in (config.first_block_number..=last_block_number).step_by(config.step as usize)
    {
        let end_block = (start_block + config.step - 1).min(last_block_number);
        let block_count = (end_block - start_block + 1) as usize;

        info!("Processing blocks {} to {} ({} blocks)", start_block, end_block, block_count);

        let mut block_tuples: Vec<BlockTuple> = Vec::new();
        let mut offsets: Vec<i64> = Vec::new();
        let mut position: i64 = 0;

        for block_number in start_block..=end_block {
            let header = provider
                .header_by_number(block_number)?
                .ok_or_else(|| eyre!("Header not found for block {}", block_number))?;

            let body = provider
                .block_by_number(block_number)?
                .ok_or_else(|| eyre!("Block body not found for block {}", block_number))?;

            let receipts = provider
                .receipts_by_block(block_number.into())?
                .ok_or_else(|| eyre!("Receipts not found for block {}", block_number))?;

            total_difficulty += header.difficulty();

            let compressed_header = CompressedHeader::from_header(&header.into())?;
            let compressed_body = CompressedBody::from_body(&body.into())?;
            let compressed_receipts = CompressedReceipts::from_encodable_list(&receipts)
                .map_err(|e| eyre!("Failed to compress receipts: {}", e))?;

            let difficulty = TotalDifficulty::new(total_difficulty);

            let block_tuple = BlockTuple::new(
                compressed_header,
                compressed_body,
                compressed_receipts,
                difficulty,
            );

            block_tuples.push(block_tuple);
            offsets.push(position);

            // TODO: calculate accurate file offsets when writing era files
            position += 100;
            total_blocks_processed += 1;

            if last_report_time.elapsed() >= report_interval {
                info!(
                    "Export progress: block {}/{} ({:.2}%) - elapsed: {:?}",
                    block_number,
                    last_block_number,
                    (total_blocks_processed as f64) /
                        ((last_block_number - config.first_block_number + 1) as f64) *
                        100.0,
                    start_time.elapsed()
                );
                last_report_time = Instant::now();
            }
        }
        if !block_tuples.is_empty() {
            export_data.push(EraExportData {
                block_tuples,
                start_block,
                network: config.network.clone(),
                offsets,
            });
        }
    }

    info!(
        "Export data preparation complete. Processed {} blocks in {:?}",
        total_blocks_processed,
        start_time.elapsed()
    );

    Ok(export_data)
}
