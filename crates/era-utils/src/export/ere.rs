//! `.ere` block-history writer.

use super::{ChunkAccumulator, EraBlockWriter, ExportBlock};
use crate::Ere;
use alloy_consensus::{BlockHeader, TxType};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use eyre::{eyre, Result};
use reth_era::{
    common::file_ops::{EraFileFormat, EraFileId, StreamWriter},
    e2s::types::Header,
    ere::{
        file::{EreFile, EreWriter},
        types::{
            execution::{
                Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedSlimReceipts,
                HeaderRecord, SlimReceipt, TotalDifficulty, MAX_BLOCKS_PER_ERE,
            },
            group::{DynamicBlockIndex, EreGroup, EreId, EreProfile},
        },
    },
};
use reth_primitives_traits::Receipt;
use std::path::{Path, PathBuf};

/// Every exported block carries header + body + receipts + total-difficulty (proofs are omitted),
/// so the block index stores four offsets per block.
const COMPONENT_COUNT: u64 = 4;

impl EraBlockWriter for Ere {
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
        let tuples = blocks.iter().map(compress_block).collect::<Result<Vec<_>>>()?;
        let accumulator = super::accumulator::<Accumulator, _, _, _>(blocks)?;
        let id = file_id(network, max_blocks_per_file, blocks)?;
        let index = block_index(blocks[0].header.number(), &tuples, &accumulator);

        let file_path = dir.join(id.to_file_name());
        let group = EreGroup::new(tuples, Some(accumulator), index);

        EreWriter::new(std::fs::File::create(&file_path)?)
            .write_file(&EreFile::new(group, id))
            .map_err(|e| eyre!("Failed to write ERE file {file_path:?}: {e}"))?;

        Ok(file_path)
    }
}

/// Compresses one block into an `ere` [`BlockTuple`] (header, body, slim receipts, cumulative total
/// difficulty).
///
/// The optional ERE `Proof` (a Portal Network header-inclusion proof) is not produced here, so the
/// file carries the [`EreProfile::NoProofs`] profile.
fn compress_block<H, B, R>(block: &ExportBlock<H, B, R>) -> Result<BlockTuple>
where
    H: BlockHeader + Encodable,
    B: Encodable,
    R: Receipt,
{
    let header = CompressedHeader::from_header(&block.header)?;
    let body = CompressedBody::from_body(&block.body)?;

    let slim_receipts = block
        .receipts
        .iter()
        .map(|receipt| {
            Ok(SlimReceipt {
                tx_type: TxType::try_from(receipt.ty())
                    .map_err(|e| eyre!("Unexpected transaction type in receipt: {e}"))?,
                status: receipt.status_or_post_state(),
                cumulative_gas_used: receipt.cumulative_gas_used(),
                logs: receipt.logs().to_vec(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let receipts = CompressedSlimReceipts::from_receipts(&slim_receipts)
        .map_err(|e| eyre!("Failed to compress receipts: {e}"))?;

    Ok(BlockTuple::new(header, body)
        .with_receipts(receipts)
        .with_total_difficulty(TotalDifficulty::new(block.total_difficulty)))
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

/// Builds the file identifier.
///
/// Per the [`EreId`] contract, the short hash is the first four bytes of the last block's hash.
fn file_id<H: BlockHeader, B, R>(
    network: &str,
    max_blocks_per_file: u64,
    blocks: &[ExportBlock<H, B, R>],
) -> Result<EreId> {
    let last_block_hash =
        blocks.last().ok_or_else(|| eyre!("cannot build ERE file id from empty block range"))?;
    let file_hash = super::short_hash(last_block_hash.block_hash);
    let id = EreId::new(network, blocks[0].header.number(), blocks.len() as u32)
        .with_hash(file_hash)
        .with_profile(EreProfile::NoProofs);
    // Custom block-per-file exports tag the era count into the filename.
    Ok(if max_blocks_per_file == MAX_BLOCKS_PER_ERE as u64 { id } else { id.with_era_count() })
}

/// Builds the [`DynamicBlockIndex`] for the file's sectioned layout.
///
/// `ere` groups records by type, so the file is laid out (after the version record) as: all
/// headers, all bodies, all receipts, all total-difficulties, the accumulator, then the index.
/// Offsets are negative `i64`s relative to the index record, per the spec's backward-pointing
/// convention.
fn block_index(
    start_block: u64,
    tuples: &[BlockTuple],
    accumulator: &Accumulator,
) -> DynamicBlockIndex {
    // Absolute position of each block's components, walked section by section.
    let mut position = Header::SIZE as i64; // past the leading version record
    let mut section = |sizes: &mut dyn Iterator<Item = i64>| {
        let mut starts = Vec::with_capacity(tuples.len());
        for size in sizes {
            starts.push(position);
            position += size;
        }
        starts
    };

    let header_pos = section(&mut tuples.iter().map(|t| t.header.to_entry().size() as i64));
    let body_pos = section(&mut tuples.iter().map(|t| t.body.to_entry().size() as i64));
    let receipts_pos =
        section(&mut tuples.iter().map(|t| entry_size(t.receipts.as_ref().map(|r| r.to_entry()))));
    let difficulty_pos = section(
        &mut tuples.iter().map(|t| entry_size(t.total_difficulty.as_ref().map(|d| d.to_entry()))),
    );

    position += accumulator.to_entry().size() as i64;
    let index_position = position;

    let mut offsets = Vec::with_capacity(tuples.len() * COMPONENT_COUNT as usize);
    for i in 0..tuples.len() {
        offsets.push(header_pos[i] - index_position);
        offsets.push(body_pos[i] - index_position);
        offsets.push(receipts_pos[i] - index_position);
        offsets.push(difficulty_pos[i] - index_position);
    }

    DynamicBlockIndex::new(start_block, COMPONENT_COUNT, offsets)
}

/// Serialized size of an optional record, or `0` when absent.
fn entry_size(entry: Option<reth_era::e2s::types::Entry>) -> i64 {
    entry.map_or(0, |e| e.size() as i64)
}
