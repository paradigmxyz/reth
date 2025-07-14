//! Era1 types
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

use crate::{
    e2s_types::{E2sError, Entry},
    execution_types::{Accumulator, BlockTuple, MAX_BLOCKS_PER_ERA1},
};
use alloy_primitives::BlockNumber;

/// `BlockIndex` record: ['i', '2']
pub const BLOCK_INDEX: [u8; 2] = [0x66, 0x32];

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
    pub const fn new(
        blocks: Vec<BlockTuple>,
        accumulator: Accumulator,
        block_index: BlockIndex,
    ) -> Self {
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
    pub const fn new(starting_number: BlockNumber, offsets: Vec<i64>) -> Self {
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

    /// Optional hash identifier for this file
    /// First 4 bytes of the last historical root in the last state in the era file
    pub hash: Option<[u8; 4]>,
}

impl Era1Id {
    /// Create a new [`Era1Id`]
    pub fn new(
        network_name: impl Into<String>,
        start_block: BlockNumber,
        block_count: u32,
    ) -> Self {
        Self { network_name: network_name.into(), start_block, block_count, hash: None }
    }

    /// Add a hash identifier to  [`Era1Id`]
    pub const fn with_hash(mut self, hash: [u8; 4]) -> Self {
        self.hash = Some(hash);
        self
    }

    /// Convert to file name following the era file naming:
    /// `<config-name>-<era-number>-<era-count>-<short-historical-root>.era(1)`
    /// <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md#file-name>
    /// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>
    pub fn to_file_name(&self) -> String {
        // Find which era the first block belongs to
        let era_number = self.start_block / MAX_BLOCKS_PER_ERA1 as u64;
        let era_count = self.calculate_era_count(era_number);
        if let Some(hash) = self.hash {
            format!(
                "{}-{:05}-{:05}-{:02x}{:02x}{:02x}{:02x}.era1",
                self.network_name, era_number, era_count, hash[0], hash[1], hash[2], hash[3]
            )
        } else {
            // era spec format with placeholder hash when no hash available
            // Format: `<config-name>-<era-number>-<era-count>-00000000.era1`
            format!("{}-{:05}-{:05}-00000000.era1", self.network_name, era_number, era_count)
        }
    }

    // Helper function to calculate the number of eras per era1 file,
    // If the user can decide how many blocks per era1 file there are, we need to calculate it.
    // Most of the time it should be 1, but it can never be more than 2 eras per file
    // as there is a maximum of 8192 blocks per era1 file.
    const fn calculate_era_count(&self, first_era: u64) -> u64 {
        // Calculate the actual last block number in the range
        let last_block = self.start_block + self.block_count as u64 - 1;
        // Find which era the last block belongs to
        let last_era = last_block / MAX_BLOCKS_PER_ERA1 as u64;
        // Count how many eras we span
        last_era - first_era + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_types::{
        CompressedBody, CompressedHeader, CompressedReceipts, TotalDifficulty,
    };
    use alloy_primitives::{B256, U256};

    /// Helper function to create a sample block tuple
    fn create_sample_block(data_size: usize) -> BlockTuple {
        // Create a compressed header with very sample data
        let header_data = vec![0xAA; data_size];
        let header = CompressedHeader::new(header_data);

        // Create a compressed body
        let body_data = vec![0xBB; data_size * 2];
        let body = CompressedBody::new(body_data);

        // Create compressed receipts
        let receipts_data = vec![0xCC; data_size];
        let receipts = CompressedReceipts::new(receipts_data);

        let difficulty = TotalDifficulty::new(U256::from(data_size));

        // Create and return the block tuple
        BlockTuple::new(header, body, receipts, difficulty)
    }

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

    #[test]
    fn test_era1_group_basic_construction() {
        let blocks =
            vec![create_sample_block(10), create_sample_block(15), create_sample_block(20)];

        let root_bytes = [0xDD; 32];
        let accumulator = Accumulator::new(B256::from(root_bytes));
        let block_index = BlockIndex::new(1000, vec![100, 200, 300]);

        let era1_group = Era1Group::new(blocks, accumulator.clone(), block_index);

        // Verify initial state
        assert_eq!(era1_group.blocks.len(), 3);
        assert_eq!(era1_group.other_entries.len(), 0);
        assert_eq!(era1_group.accumulator.root, accumulator.root);
        assert_eq!(era1_group.block_index.starting_number, 1000);
        assert_eq!(era1_group.block_index.offsets, vec![100, 200, 300]);
    }

    #[test]
    fn test_era1_group_add_entries() {
        let blocks = vec![create_sample_block(10)];

        let root_bytes = [0xDD; 32];
        let accumulator = Accumulator::new(B256::from(root_bytes));

        let block_index = BlockIndex::new(1000, vec![100]);

        // Create and verify group
        let mut era1_group = Era1Group::new(blocks, accumulator, block_index);
        assert_eq!(era1_group.other_entries.len(), 0);

        // Create custom entries with different types
        let entry1 = Entry::new([0x01, 0x01], vec![1, 2, 3, 4]);
        let entry2 = Entry::new([0x02, 0x02], vec![5, 6, 7, 8]);

        // Add those entries
        era1_group.add_entry(entry1);
        era1_group.add_entry(entry2);

        // Verify entries were added correctly
        assert_eq!(era1_group.other_entries.len(), 2);
        assert_eq!(era1_group.other_entries[0].entry_type, [0x01, 0x01]);
        assert_eq!(era1_group.other_entries[0].data, vec![1, 2, 3, 4]);
        assert_eq!(era1_group.other_entries[1].entry_type, [0x02, 0x02]);
        assert_eq!(era1_group.other_entries[1].data, vec![5, 6, 7, 8]);
    }

    #[test]
    fn test_era1_group_with_mismatched_index() {
        let blocks =
            vec![create_sample_block(10), create_sample_block(15), create_sample_block(20)];

        let root_bytes = [0xDD; 32];
        let accumulator = Accumulator::new(B256::from(root_bytes));

        // Create block index with different starting number
        let block_index = BlockIndex::new(2000, vec![100, 200, 300]);

        // This should create a valid Era1Group
        // even though the block numbers don't match the block index
        // validation not at the era1 group level
        let era1_group = Era1Group::new(blocks, accumulator, block_index);

        // Verify the mismatch exists but the group was created
        assert_eq!(era1_group.blocks.len(), 3);
        assert_eq!(era1_group.block_index.starting_number, 2000);
    }

    #[test_case::test_case(
        Era1Id::new("mainnet", 0, 8192).with_hash([0x5e, 0xc1, 0xff, 0xb8]),
        "mainnet-00000-00001-5ec1ffb8.era1";
        "Mainnet era 0"
    )]
    #[test_case::test_case(
        Era1Id::new("mainnet", 8192, 8192).with_hash([0x5e, 0xcb, 0x9b, 0xf9]),
        "mainnet-00001-00001-5ecb9bf9.era1";
        "Mainnet era 1"
    )]
    #[test_case::test_case(
        Era1Id::new("sepolia", 0, 8192).with_hash([0x90, 0x91, 0x84, 0x72]),
        "sepolia-00000-00001-90918472.era1";
        "Sepolia era 0"
    )]
    #[test_case::test_case(
        Era1Id::new("sepolia", 155648, 8192).with_hash([0xfa, 0x77, 0x00, 0x19]),
        "sepolia-00019-00001-fa770019.era1";
        "Sepolia era 19"
    )]
    #[test_case::test_case(
        Era1Id::new("mainnet", 1000, 100),
        "mainnet-00000-00001-00000000.era1";
        "ID without hash"
    )]
    #[test_case::test_case(
        Era1Id::new("sepolia", 101130240, 8192).with_hash([0xab, 0xcd, 0xef, 0x12]),
        "sepolia-12345-00001-abcdef12.era1";
        "Large block number era 12345"
    )]
    fn test_era1id_file_naming(id: Era1Id, expected_file_name: &str) {
        let actual_file_name = id.to_file_name();
        assert_eq!(actual_file_name, expected_file_name);
    }
}
