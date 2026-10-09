//! Mutable header utilities.

use crate::BlockHeader;
use alloy_consensus::Header;
use alloy_primitives::{B256, BlockHash, BlockNumber, Bytes, U256};

/// A helper trait for [`Header`]s that allows for mutable access to the headers values.
///
/// This allows for modifying the header for testing and mocking purposes.
pub trait HeaderMut: BlockHeader {
    /// Updates the parent block hash.
    fn set_parent_hash(&mut self, hash: BlockHash);

    /// Updates the block number.
    fn set_block_number(&mut self, number: BlockNumber);

    /// Updates the block's timestamp.
    fn set_timestamp(&mut self, timestamp: u64);

    /// Updates the block state root.
    fn set_state_root(&mut self, state_root: B256);

    /// Updates the block difficulty.
    fn set_difficulty(&mut self, difficulty: U256);

    /// Updates the mix hash.
    fn set_mix_hash(&mut self, mix_hash: B256);

    /// Updates the extra data.
    fn set_extra_data(&mut self, extra_data: Bytes);

    /// Updates the parent beacon block root.
    fn set_parent_beacon_block_root(&mut self, parent_beacon_block_root: Option<B256>);

    /// Updates the block number (alias for CLI compatibility).
    #[inline]
    fn set_number(&mut self, number: u64) {
        self.set_block_number(number);
    }
}

impl HeaderMut for Header {
    #[inline]
    fn set_parent_hash(&mut self, hash: BlockHash) {
        self.parent_hash = hash;
    }

    #[inline]
    fn set_block_number(&mut self, number: BlockNumber) {
        self.number = number;
    }

    #[inline]
    fn set_timestamp(&mut self, timestamp: u64) {
        self.timestamp = timestamp;
    }

    #[inline]
    fn set_state_root(&mut self, state_root: B256) {
        self.state_root = state_root;
    }

    #[inline]
    fn set_difficulty(&mut self, difficulty: U256) {
        self.difficulty = difficulty;
    }

    #[inline]
    fn set_mix_hash(&mut self, mix_hash: B256) {
        self.mix_hash = mix_hash;
    }

    #[inline]
    fn set_extra_data(&mut self, extra_data: Bytes) {
        self.extra_data = extra_data;
    }

    #[inline]
    fn set_parent_beacon_block_root(&mut self, parent_beacon_block_root: Option<B256>) {
        self.parent_beacon_block_root = parent_beacon_block_root;
    }
}
