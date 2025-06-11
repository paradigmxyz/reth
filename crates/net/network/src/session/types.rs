//! Shared types for network sessions.

use alloy_primitives::B256;
use parking_lot::RwLock;
use reth_eth_wire::BlockRangeUpdate;
use std::{
    ops::RangeInclusive,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// Information about the range of blocks available from a peer.
///
/// This represents the announced `eth69`
/// [`BlockRangeUpdate`] of a peer.
#[derive(Debug, Clone)]
pub struct BlockRangeInfo {
    /// The inner range information.
    inner: Arc<BlockRangeInfoInner>,
}

impl BlockRangeInfo {
    /// Creates a new range information.
    pub fn new(earliest: u64, latest: u64, latest_hash: B256) -> Self {
        Self {
            inner: Arc::new(BlockRangeInfoInner {
                earliest: AtomicU64::new(earliest),
                latest: AtomicU64::new(latest),
                latest_hash: RwLock::new(latest_hash),
            }),
        }
    }

    /// Returns true if the block number is within the range of blocks available from the peer.
    pub fn contains(&self, block_number: u64) -> bool {
        self.range().contains(&block_number)
    }

    /// Returns the range of blocks available from the peer.
    pub fn range(&self) -> RangeInclusive<u64> {
        let earliest = self.earliest();
        let latest = self.latest();
        RangeInclusive::new(earliest, latest)
    }

    /// Returns the earliest block number available from the peer.
    pub fn earliest(&self) -> u64 {
        self.inner.earliest.load(Ordering::Relaxed)
    }

    /// Returns the latest block number available from the peer.
    pub fn latest(&self) -> u64 {
        self.inner.latest.load(Ordering::Relaxed)
    }

    /// Returns the latest block hash available from the peer.
    pub fn latest_hash(&self) -> B256 {
        *self.inner.latest_hash.read()
    }

    /// Updates the range information.
    pub fn update(&self, earliest: u64, latest: u64, latest_hash: B256) {
        self.inner.earliest.store(earliest, Ordering::Relaxed);
        self.inner.latest.store(latest, Ordering::Relaxed);
        *self.inner.latest_hash.write() = latest_hash;
    }

    /// Converts the current range information to an Eth69 [`BlockRangeUpdate`] message.
    pub fn to_message(&self) -> BlockRangeUpdate {
        BlockRangeUpdate {
            earliest: self.earliest(),
            latest: self.latest(),
            latest_hash: self.latest_hash(),
        }
    }
}

/// Inner structure containing the range information with atomic and thread-safe fields.
#[derive(Debug)]
pub(crate) struct BlockRangeInfoInner {
    /// The earliest block which is available.
    earliest: AtomicU64,
    /// The latest block which is available.
    latest: AtomicU64,
    /// Latest available block's hash.
    latest_hash: RwLock<B256>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_block_range_info_to_message() {
        let earliest = 100;
        let latest = 200;
        let hash = B256::from([1; 32]);

        let range_info = BlockRangeInfo::new(earliest, latest, hash);
        let message = range_info.to_message();

        assert_eq!(message.earliest, earliest);
        assert_eq!(message.latest, latest);
        assert_eq!(message.latest_hash, hash);
    }

    #[test]
    fn test_block_range_info_update_and_to_message() {
        let range_info = BlockRangeInfo::new(0, 0, B256::ZERO);

        let new_earliest = 500;
        let new_latest = 1000;
        let new_hash = B256::from([42; 32]);

        range_info.update(new_earliest, new_latest, new_hash);
        let message = range_info.to_message();

        assert_eq!(message.earliest, new_earliest);
        assert_eq!(message.latest, new_latest);
        assert_eq!(message.latest_hash, new_hash);
    }
}
