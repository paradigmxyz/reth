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
        // Total difficulty and the accumulator are pre-merge only: post-merge blocks have zero
        // difficulty and the accumulator is frozen at the merge. Difficulty drops to zero
        // monotonically, so the first block decides the whole file.
        let pre_merge = !blocks[0].header.difficulty().is_zero();

        let tuples = blocks
            .iter()
            .map(|block| compress_block(block, pre_merge))
            .collect::<Result<Vec<_>>>()?;
        let accumulator =
            pre_merge.then(|| super::accumulator::<Accumulator, _, _, _>(blocks)).transpose()?;
        let id = file_id(network, max_blocks_per_file, blocks)?;
        let index = block_index(blocks[0].header.number(), &tuples, accumulator.as_ref());

        let file_path = dir.join(id.to_file_name());
        let group = EreGroup::new(tuples, accumulator, index);

        EreWriter::new(std::fs::File::create(&file_path)?)
            .write_file(&EreFile::new(group, id))
            .map_err(|e| eyre!("Failed to write ERE file {file_path:?}: {e}"))?;

        Ok(file_path)
    }
}

/// Compresses one block into an `ere` [`BlockTuple`] (header, body, slim receipts, and, for
/// pre-merge blocks, cumulative total difficulty).
///
/// The optional ERE `Proof` (a Portal Network header-inclusion proof) is not produced here, so the
/// file carries the [`EreProfile::NoProofs`] profile.
fn compress_block<H, B, R>(
    block: &ExportBlock<H, B, R>,
    include_total_difficulty: bool,
) -> Result<BlockTuple>
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

    let tuple = BlockTuple::new(header, body).with_receipts(receipts);
    Ok(if include_total_difficulty {
        tuple.with_total_difficulty(TotalDifficulty::new(block.total_difficulty))
    } else {
        tuple
    })
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
/// headers, all bodies, all receipts, all total-difficulties and the accumulator (both pre-merge
/// only), then the index. Offsets are negative `i64`s relative to the index record, per the spec's
/// backward-pointing convention.
fn block_index(
    start_block: u64,
    tuples: &[BlockTuple],
    accumulator: Option<&Accumulator>,
) -> DynamicBlockIndex {
    // Every block carries header + body + receipts, plus total-difficulty for pre-merge files
    // (proofs are always omitted), so the index stores three or four offsets per block.
    let has_total_difficulty = tuples.first().is_some_and(|t| t.total_difficulty.is_some());
    let component_count = if has_total_difficulty { 4 } else { 3 };

    // Absolute position of each block's components, walked section by section, starting past the
    // leading version record.
    let mut position = Header::SIZE as i64;
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
    let difficulty_pos = has_total_difficulty.then(|| {
        section(
            &mut tuples
                .iter()
                .map(|t| entry_size(t.total_difficulty.as_ref().map(|d| d.to_entry()))),
        )
    });

    if let Some(accumulator) = accumulator {
        position += accumulator.to_entry().size() as i64;
    }
    let index_position = position;

    let mut offsets = Vec::with_capacity(tuples.len() * component_count as usize);
    for i in 0..tuples.len() {
        offsets.push(header_pos[i] - index_position);
        offsets.push(body_pos[i] - index_position);
        offsets.push(receipts_pos[i] - index_position);
        if let Some(difficulty_pos) = &difficulty_pos {
            offsets.push(difficulty_pos[i] - index_position);
        }
    }

    DynamicBlockIndex::new(start_block, component_count, offsets)
}

/// Serialized size of an optional record, or `0` when absent.
fn entry_size(entry: Option<reth_era::e2s::types::Entry>) -> i64 {
    entry.map_or(0, |e| e.size() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use reth_era::{common::file_ops::StreamReader, ere::file::EreReader};
    use reth_ethereum_primitives::{BlockBody, Receipt as EthReceipt};
    use tempfile::tempdir;

    /// `count` empty blocks sharing one difficulty, so the chunk is uniformly pre- or post-merge.
    fn export_blocks(
        count: u64,
        difficulty: U256,
    ) -> Vec<ExportBlock<Header, BlockBody, EthReceipt>> {
        (0..count)
            .map(|number| ExportBlock {
                header: Header { number, difficulty, ..Default::default() },
                block_hash: B256::repeat_byte(number as u8 + 1),
                body: BlockBody::default(),
                receipts: Vec::new(),
                total_difficulty: U256::from(number + 1),
            })
            .collect()
    }

    fn write_and_read(blocks: &[ExportBlock<Header, BlockBody, EthReceipt>]) -> EreFile {
        let dir = tempdir().unwrap();
        let path =
            Ere::write_file("mainnet", MAX_BLOCKS_PER_ERE as u64, blocks, dir.path()).unwrap();
        EreReader::new(std::fs::File::open(path).unwrap()).read("mainnet".to_string()).unwrap()
    }

    #[test]
    fn pre_merge_file_carries_total_difficulty_and_accumulator() {
        let file = write_and_read(&export_blocks(3, U256::from(1)));

        assert!(file.group.accumulator.is_some());
        assert_eq!(file.group.index.component_count(), 4);
        assert!(file.group.blocks.iter().all(|b| b.total_difficulty.is_some()));
    }

    #[test]
    fn post_merge_file_omits_total_difficulty_and_accumulator() {
        let file = write_and_read(&export_blocks(3, U256::ZERO));

        assert!(file.group.accumulator.is_none());
        assert_eq!(file.group.index.component_count(), 3);
        assert!(file.group.blocks.iter().all(|b| b.total_difficulty.is_none()));
    }
}
