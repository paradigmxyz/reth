#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! A [Consensus](reth_interfaces::consensus::Consensus) implementation for local testing purposes
//! that automatically seals blocks.

use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, Header, PeerId, H256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
mod client;
mod mode;
mod task;

/// A consensus implementation that follows a strategy for announcing blocks via [ForkchoiceState]
#[derive(Debug)]
pub struct AutoSealConsensus {}

/// In memory storage
#[derive(Debug, Clone, Default)]
pub(crate) struct Storage {
    inner: Arc<RwLock<StorageInner>>,
}

// == impl Storage ===

impl Storage {
    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner> {
        self.inner.write().await
    }

    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner> {
        self.inner.read().await
    }
}

#[derive(Default, Debug)]
pub(crate) struct StorageInner {
    /// Headers buffered for download.
    pub(crate) headers: HashMap<BlockNumber, Header>,
    /// A mapping between block hash and number.
    pub(crate) hash_to_number: HashMap<BlockHash, BlockNumber>,
    /// Bodies buffered for download.
    pub(crate) bodies: HashMap<BlockHash, BlockBody>,
    /// Tracks best block
    pub(crate) best_block: u64,
    /// Tracks hash of best block
    pub(crate) best_hash: H256,
}

// === impl StorageInner ===

impl StorageInner {
    /// Returns the block hash for the given block number if it exists.
    pub(crate) fn block_hash(&self, num: u64) -> Option<BlockHash> {
        self.hash_to_number.iter().find_map(|(k, v)| num.eq(v).then(*k))
    }

    /// Returns the matching header if it exists.
    pub(crate) fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> Option<Header> {
        let num = match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.hash_to_number.get(&hash).copied()?,
            BlockHashOrNumber::Number(num) => num,
        };
        self.headers.get(&num).cloned()
    }
}
