//! Era1 types
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

use crate::{
    e2s_types::{E2sError, Entry, BLOCK_INDEX},
    execution_types::{Accumulator, BlockTuple},
};
use alloy_primitives::BlockNumber;

/// File content in an Era1 file
///
/// Format: `block-tuple* | other-entries* | Accumulator | BlockIndex`
#[derive(Debug)]
pub struct Era1Group {
    /// Blocks in this era1 group
    pub blocks: Vec<BlockTuple>,

    /// Other entries that don't fit into the standard categories
    pub other_entries: Vec<Entry>,

    /// Accumulator is hash tree root of block headers and difficulties
    pub accumulator: Accumulator,

    /// Block index, optional, omitted for genesis era
    pub block_index: BlockIndex,
}

impl Era1Group {
    /// Create a new [`Era1Group`]
    pub fn new(blocks: Vec<BlockTuple>, accumulator: Accumulator, block_index: BlockIndex) -> Self {
        Self { blocks, accumulator, block_index, other_entries: Vec::new() }
    }
    /// Add another entry to this group
    pub fn add_entry(&mut self, entry: Entry) {
        self.other_entries.push(entry);
    }
}

/// [`BlockIndex`] records store offsets to data at specific block numbers
/// from the beginning of the index record to the beginning of the corresponding data.
///
/// Format:
/// `starting-(block)-number | index | index | index ... | count`
#[derive(Debug, Clone)]
pub struct BlockIndex {
    /// Starting block number
    pub starting_number: BlockNumber,

    /// Offsets to data at each block number
    pub offsets: Vec<i64>,
}

impl BlockIndex {
    /// Create a new [`BlockIndex`]
    pub fn new(starting_number: BlockNumber, offsets: Vec<i64>) -> Self {
        Self { starting_number, offsets }
    }

    /// Get the offset for a specific block number
    pub fn offset_for_block(&self, block_number: BlockNumber) -> Option<i64> {
        if block_number < self.starting_number {
            return None;
        }

        let index = (block_number - self.starting_number) as usize;
        self.offsets.get(index).copied()
    }

    /// Convert to an [`Entry`] for storage in an e2store file
    pub fn to_entry(&self) -> Entry {
        // Format: starting-(block)-number | index | index | index ... | count
        let mut data = Vec::with_capacity(8 + self.offsets.len() * 8 + 8);

        // Add starting block number
        data.extend_from_slice(&self.starting_number.to_le_bytes());

        // Add all offsets
        for offset in &self.offsets {
            data.extend_from_slice(&offset.to_le_bytes());
        }

        // Add count
        data.extend_from_slice(&(self.offsets.len() as i64).to_le_bytes());

        Entry::new(BLOCK_INDEX, data)
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != BLOCK_INDEX {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for BlockIndex: expected {:02x}{:02x}, got {:02x}{:02x}",
                BLOCK_INDEX[0], BLOCK_INDEX[1], entry.entry_type[0], entry.entry_type[1]
            )));
        }

        if entry.data.len() < 16 {
            return Err(E2sError::Ssz(String::from(
                "BlockIndex entry too short to contain starting block number and count",
            )));
        }

        // Extract starting block number = first 8 bytes
        let mut starting_number_bytes = [0u8; 8];
        starting_number_bytes.copy_from_slice(&entry.data[0..8]);
        let starting_number = u64::from_le_bytes(starting_number_bytes);

        // Extract count = last 8 bytes
        let mut count_bytes = [0u8; 8];
        count_bytes.copy_from_slice(&entry.data[entry.data.len() - 8..]);
        let count = u64::from_le_bytes(count_bytes) as usize;

        // Verify that the entry has the correct size
        let expected_size = 8 + count * 8 + 8;
        if entry.data.len() != expected_size {
            return Err(E2sError::Ssz(format!(
                "BlockIndex entry has incorrect size: expected {}, got {}",
                expected_size,
                entry.data.len()
            )));
        }

        // Extract all offsets
        let mut offsets = Vec::with_capacity(count);
        for i in 0..count {
            let start = 8 + i * 8;
            let end = start + 8;
            let mut offset_bytes = [0u8; 8];
            offset_bytes.copy_from_slice(&entry.data[start..end]);
            offsets.push(i64::from_le_bytes(offset_bytes));
        }

        Ok(Self { starting_number, offsets })
    }
}

/// Era1 file identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Era1Id {
    /// Network configuration name
    pub network_name: String,

    /// First block number in file
    pub start_block: BlockNumber,

    /// Number of blocks in the file
    pub block_count: u32,
}

impl Era1Id {
    /// Create a new [`Era1Id`]
    pub fn new(
        network_name: impl Into<String>,
        start_block: BlockNumber,
        block_count: u32,
    ) -> Self {
        Self { network_name: network_name.into(), start_block, block_count }
    }

    /// Convert to file name following the era1 file naming:
    /// `<network-name>-<start-block>-<block-count>.era1`
    /// inspired from era file naming convention in
    /// <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md#file-name>
    /// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>
    pub fn to_file_name(&self) -> String {
        format!("{}-{}-{}.era1", self.network_name, self.start_block, self.block_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2s_types::BLOCK_INDEX;

    #[test]
    fn test_block_index_roundtrip() {
        let starting_number = 1000;
        let offsets = vec![100, 200, 300, 400, 500];

        let block_index = BlockIndex::new(starting_number, offsets.clone());

        let entry = block_index.to_entry();

        // Validate entry type
        assert_eq!(entry.entry_type, BLOCK_INDEX);

        // Convert back to block index
        let recovered = BlockIndex::from_entry(&entry).unwrap();

        // Verify fields match
        assert_eq!(recovered.starting_number, starting_number);
        assert_eq!(recovered.offsets, offsets);
    }

    #[test]
    fn test_block_index_offset_lookup() {
        let starting_number = 1000;
        let offsets = vec![100, 200, 300, 400, 500];

        let block_index = BlockIndex::new(starting_number, offsets);

        // Test valid lookups
        assert_eq!(block_index.offset_for_block(1000), Some(100));
        assert_eq!(block_index.offset_for_block(1002), Some(300));
        assert_eq!(block_index.offset_for_block(1004), Some(500));

        // Test out of range lookups
        assert_eq!(block_index.offset_for_block(999), None);
        assert_eq!(block_index.offset_for_block(1005), None);
    }
}
