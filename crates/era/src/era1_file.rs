//! Represents a complete Era1 file
//!
//! The structure of an Era1 file follows the specification:
//! `Version | block-tuple* | other-entries* | Accumulator | BlockIndex`
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

use crate::{
    e2s_types::Version,
    era1_types::{Era1Group, Era1Id},
    execution_types::BlockTuple,
};
use alloy_primitives::BlockNumber;

/// Era1 file interface
#[derive(Debug)]
pub struct Era1File {
    /// Version record, must be the first record in the file
    pub version: Version,

    /// Main content group of the Era1 file
    pub group: Era1Group,

    /// File identifier of Era1 file
    pub id: Era1Id,
}

impl Era1File {
    /// Create a new [`Era1File`]
    pub fn new(group: Era1Group, id: Era1Id) -> Self {
        Self { version: Version, group, id }
    }

    /// Get a block by its number, if present in this file
    pub fn get_block_by_number(&self, number: BlockNumber) -> Option<&BlockTuple> {
        let index = (number - self.group.block_index.starting_number) as usize;
        (index < self.group.blocks.len()).then(|| &self.group.blocks[index])
    }

    /// Get the range of block numbers contained in this file
    pub fn block_range(&self) -> std::ops::RangeInclusive<BlockNumber> {
        let start = self.group.block_index.starting_number;
        let end = start + (self.group.blocks.len() as u64) - 1;
        start..=end
    }

    /// Check if this file contains a specific block number
    pub fn contains_block(&self, number: BlockNumber) -> bool {
        self.block_range().contains(&number)
    }
}
