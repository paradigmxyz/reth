//! `.era1` block-history writer.

use super::{ChunkAccumulator, EraBlockWriter, ExportBlock};
use crate::Era1;
use alloy_consensus::{BlockHeader, TxReceipt};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use eyre::{eyre, Result};
use reth_era::{
    common::file_ops::{EraFileId, StreamWriter},
    e2s::types::{Header, IndexEntry},
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
use reth_primitives_traits::Receipt;
use std::path::{Path, PathBuf};

impl EraBlockWriter for Era1 {
    fn write_file<H, B, R>(
        network: &str,
        max_blocks_per_file: u64,
        blocks: &[ExportBlock<H, B, R>],
        dir: &Path,
    ) -> Result<PathBuf>
    where
        H: BlockHeader + Encodable,
        B: Encodable,
        R: Receipt,
    {
        let accumulator = super::accumulator::<Accumulator, _, _, _>(blocks)?;
        let file_path = dir.join(file_name(network, max_blocks_per_file, blocks, &accumulator));

        let mut writer = Era1Writer::new(std::fs::File::create(&file_path)?);
        writer.write_version()?;

        // `era1` streams its records block by block; offsets track the running write position and
        // are rebased onto the block-index record once that record's position is known.
        let mut offsets = Vec::<i64>::with_capacity(blocks.len());
        let mut position = Header::SIZE as i64; // past the leading version record
        for block in blocks {
            let tuple = compress_block(block)?;
            offsets.push(position);
            position += tuple.size() as i64;
            writer.write_block(&tuple)?;
        }

        let index_position = position + accumulator.to_entry().size() as i64;
        let relative: Vec<i64> = offsets.iter().map(|&abs| abs - index_position).collect();

        writer.write_accumulator(&accumulator)?;
        writer.write_block_index(&BlockIndex::new(blocks[0].header.number(), relative))?;
        writer.flush()?;

        Ok(file_path)
    }
}

/// Compresses one block into an `era1` [`BlockTuple`] (header, body, bloom-bearing receipts,
/// cumulative total difficulty).
fn compress_block<H, B, R>(block: &ExportBlock<H, B, R>) -> Result<BlockTuple>
where
    H: BlockHeader + Encodable,
    B: Encodable,
    R: Receipt,
{
    let header = CompressedHeader::from_header(&block.header)?;
    let body = CompressedBody::from_body(&block.body)?;
    let receipts_with_bloom: Vec<_> =
        block.receipts.iter().map(|r| TxReceipt::with_bloom_ref(r)).collect();
    let receipts = CompressedReceipts::from_encodable_list(&receipts_with_bloom)
        .map_err(|e| eyre!("Failed to compress receipts: {e}"))?;

    Ok(BlockTuple::new(header, body, receipts, TotalDifficulty::new(block.total_difficulty)))
}

impl ChunkAccumulator for Accumulator {
    fn from_pairs(records: &[(B256, U256)]) -> Result<Self> {
        let records: Vec<HeaderRecord> = records
            .iter()
            .map(|&(block_hash, total_difficulty)| HeaderRecord { block_hash, total_difficulty })
            .collect();
        Self::from_header_records(&records).map_err(|e| eyre!("Failed to compute accumulator: {e}"))
    }
}

/// Builds the output filename, taking the short hash from the accumulator root.
fn file_name<H: BlockHeader, B, R>(
    network: &str,
    max_blocks_per_file: u64,
    blocks: &[ExportBlock<H, B, R>],
    accumulator: &Accumulator,
) -> String {
    let file_hash = super::short_hash(accumulator.root);
    let id =
        Era1Id::new(network, blocks[0].header.number(), blocks.len() as u32).with_hash(file_hash);
    // Custom block-per-file exports tag the era count into the filename.
    if max_blocks_per_file == MAX_BLOCKS_PER_ERA1 as u64 {
        id.to_file_name()
    } else {
        id.with_era_count().to_file_name()
    }
}
