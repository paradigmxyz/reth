//! Mutable header utilities.

use crate::BlockHeader;
use alloy_consensus::Header;
use alloy_primitives::{BlockHash, BlockNumber, B256, U256};

/// A helper trait for [`Header`]s that allows for mutable access to the headers values.
///
/// This allows for modifying the header for testing and mocking purposes.
pub trait HeaderMut: BlockHeader {
    /// Updates the parent block hash.
    fn set_parent_hash(&mut self, hash: BlockHash);

    /// Updates the block number.
    fn set_block_number(&mut self, number: BlockNumber);

    /// Updates the block's timestamp.
    fn set_timestamp(&mut self, number: BlockNumber);

    /// Updates the block state root.
    fn set_state_root(&mut self, state_root: B256);

    /// Updates the block difficulty.
    fn set_difficulty(&mut self, difficulty: U256);

    /// Updates the block number (alias for CLI compatibility).
    fn set_number(&mut self, number: u64) {
        self.set_block_number(number);
    }
}

impl HeaderMut for Header {
    fn set_parent_hash(&mut self, hash: BlockHash) {
        self.parent_hash = hash;
    }

    fn set_block_number(&mut self, number: BlockNumber) {
        self.number = number;
    }

    fn set_timestamp(&mut self, number: BlockNumber) {
        self.timestamp = number;
    }

    fn set_state_root(&mut self, state_root: B256) {
        self.state_root = state_root;
    }

    fn set_difficulty(&mut self, difficulty: U256) {
        self.difficulty = difficulty;
    }
}
