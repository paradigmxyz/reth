//! Support for maintaining the blob pool.

use crate::blobstore::BlobStore;
use reth_primitives::{BlockNumber, Header, H256};
use std::collections::BTreeMap;

/// The type that is used to maintain the blob store and discard finalized transactions.
#[derive(Debug)]
#[allow(unused)]
pub struct BlobStoreMaintainer<S> {
    /// The blob store that holds all the blob data.
    store: S,
    /// Keeps track of the blob transactions included in blocks.
    blob_txs_in_blocks: BTreeMap<BlockNumber, Vec<H256>>,
}

impl<S> BlobStoreMaintainer<S> {
    /// Creates a new blob store maintenance instance.
    pub fn new(store: S) -> Self {
        Self { store, blob_txs_in_blocks: Default::default() }
    }
}

impl<S: BlobStore> BlobStoreMaintainer<S> {
    /// Adds a block to the blob store maintenance.
    pub fn add_block(&mut self, block_number: BlockNumber, blob_txs: Vec<H256>) {
        self.blob_txs_in_blocks.insert(block_number, blob_txs);
    }

    /// Invoked when a block is finalized.
    pub fn on_finalized(&mut self, header: &Header) {}
}
