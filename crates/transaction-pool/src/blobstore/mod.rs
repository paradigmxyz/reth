//! Storage for blob data of EIP4844 transactions.

use reth_primitives::H256;

mod mem;
pub use mem::InMemoryBlobStore;

/// A blob store that can be used to store blob data of EIP4844 transactions.
///
/// This type is responsible for keeping track of blob data until it is no longer needed (after
/// finalization).
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync + Clone + 'static {
    // TODO add fn for fetch many, delete many, etc.
    async fn get(&self, x: H256) -> Option<()>;

    /// Data size of all transactions in the blob store.
    fn data_size(&self) -> usize;
}

// TODO add as additional param for Pool struct
// Add functions to fetch many, delete many, etc to TransactionPool trait
// TODO add  get pooled transactions to transaction pool trait that is async and returns Vec<PooledTransactionResponse>
// TODO keep track of pooled transaction responses in transaction manager, since async and we want to keep track which peers requesting blobs
// TODO soft limit on getpooledtransactions
// https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09