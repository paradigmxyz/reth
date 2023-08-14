use crate::blobstore::BlobStore;
use reth_primitives::H256;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};
use tokio::sync::RwLock;

/// An in-memory blob store.
#[derive(Clone, Debug)]
pub struct InMemoryBlobStore {
    inner: Arc<InMemoryBlobStoreInner>,
}

#[derive(Debug)]
struct InMemoryBlobStoreInner {
    /// Storage for all blob data.
    store: RwLock<HashMap<H256, ()>>,
    size: AtomicUsize,
}

// #[async_trait::async_trait]
// impl BlobStore for InMemoryBlobStore {
//
// }
