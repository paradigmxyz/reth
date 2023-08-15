//! Storage for blob data of EIP4844 transactions.

#![allow(missing_docs)]

pub use mem::InMemoryBlobStore;
use reth_primitives::{bytes::Bytes, H256};
use reth_rlp::{Decodable, RlpDecodable, RlpEncodable};
use std::collections::BTreeMap;
mod mem;

// TODO replace with the actual type
#[derive(Debug, Clone, RlpDecodable, RlpEncodable)]
pub struct BlobSideCar;

impl BlobSideCar {
    pub fn size(&self) -> usize {
        0
    }
}

/// A blob store that can be used to store blob data of EIP4844 transactions.
///
/// This type is responsible for keeping track of blob data until it is no longer needed (after
/// finalization).
///
/// Note: this is Clone because it is expected to be wrapped in an Arc.
pub trait BlobStore: Send + Sync + Clone + 'static {
    /// Inserts the blob sidecar into the store
    fn insert(&self, tx: H256, data: BlobSideCar) -> Result<(), BlobStoreError>;

    /// Inserts multiple blob sidecars into the store
    fn insert_all(&self, txs: Vec<(H256, BlobSideCar)>) -> Result<(), BlobStoreError>;

    /// Deletes the blob sidecar from the store
    fn delete(&self, tx: H256) -> Result<(), BlobStoreError>;

    /// Deletes multiple blob sidecars from the store
    fn delete_all(&self, txs: Vec<H256>) -> Result<(), BlobStoreError>;

    /// Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: H256) -> Result<Option<BlobSideCar>, BlobStoreError> {
        if let Some(raw) = self.get_raw(tx)? {
            Ok(Some(BlobSideCar::decode(&mut raw.as_ref())?))
        } else {
            Ok(None)
        }
    }

    /// Retrieves all decoded blob data for the given transaction hashes
    fn get_all(&self, txs: Vec<H256>) -> Result<Vec<(H256, BlobSideCar)>, BlobStoreError>;

    /// Retrieves the raw blob data for the given transaction hash.
    fn get_raw(&self, tx: H256) -> Result<Option<Bytes>, BlobStoreError>;

    /// Data size of all transactions in the blob store.
    fn data_size(&self) -> usize;
}

/// Error variants that can occur when interacting with a blob store.
#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    /// Failed to decode the stored blob data.
    #[error("failed to decode blob data: {0}")]
    DecodeError(#[from] reth_rlp::DecodeError),
    /// Other implementation specific error.
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// The type that is used to maintain the blob store and discard finalized transactions.
#[derive(Debug)]
pub struct BlobStoreMaintenance<S> {
    store: S,
    /// Keeps track of the blob transactions that are in blocks.
    blob_txs_in_blocks: BTreeMap<u64, Vec<H256>>,
}

impl<S> BlobStoreMaintenance<S> {
    /// Creates a new blob store maintenance instance.
    pub fn new(store: S) -> Self {
        Self { store, blob_txs_in_blocks: Default::default() }
    }
}

impl<S: BlobStore> BlobStoreMaintenance<S> {
    /// Invoked when a block is finalized.
    pub fn on_finalized(&mut self, _block_number: u64) {}
}

// TODO add as additional param for Pool struct
// Add functions to fetch many, delete many, etc to TransactionPool trait
// TODO add  get pooled transactions to transaction pool trait that is async and returns
// Vec<PooledTransactionResponse> TODO keep track of pooled transaction responses in transaction
// manager, since async and we want to keep track which peers requesting blobs TODO soft limit on
// getpooledtransactions https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused)]
    struct DynStore {
        store: Box<dyn BlobStore>,
    }
}
