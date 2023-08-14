//! Storage for blob data of EIP4844 transactions.


use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use reth_primitives::H256;

/// A blob store that can be used to store blob data of EIP4844 transactions.
///
/// This type is responsible for keeping track of blob data until it is no longer needed (after finalization).
///
/// TODO in memory, file...
pub trait BlobStore: Send + Sync + 'static {

    // TODO add fns async
    // get,store,delete
    fn get(&mut self, x: H256) -> Option<()>;

    /// Data size of all transactions in the blob store.
    fn data_size(&self) -> usize;
}

/// The handle that can be used to send requests to the blob storage service.
#[derive(Clone, Debug)]
pub struct BlobStorageHandle {
    // everything that needs blob data needs this
    to_service: mpsc::UnboundedSender<BlobStorageRequest>
}


// EIP4844 blob transaction storage service task.
#[must_use = "blob storage service must be spawned for it to do anything"]
pub struct BlobStorageService<S: BlobStore> {
    /// where the blob data is stored.
    store: S,

    // TODO add cache

    /// Clone of the sender side of the channel.
    ///
    /// This ensures the channel is not closed until the service is dropped.
    to_service: mpsc::UnboundedSender<BlobStorageRequest>,
    /// Receiver stream of incoming requests.
    incoming_requests: ReceiverStream<BlobStorageRequest>
}

impl<S: BlobStore> Future for BlobStorageService<S> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
          // TODO handle incoming requests
        }
    }
}


enum BlobStorageRequest {
    // TODO add lookup requests
    Delete {
        /// Hash of the blob transaction to delete.
        tx: H256
    }
}