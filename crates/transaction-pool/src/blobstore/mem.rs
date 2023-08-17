use crate::blobstore::{BlobStore, BlobStoreError, BlobTransactionSidecar};
use parking_lot::RwLock;
use reth_primitives::H256;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};

/// An in-memory blob store.
#[derive(Clone, Debug, Default)]
pub struct InMemoryBlobStore {
    inner: Arc<InMemoryBlobStoreInner>,
}

#[derive(Debug, Default)]
struct InMemoryBlobStoreInner {
    /// Storage for all blob data.
    store: RwLock<HashMap<H256, BlobTransactionSidecar>>,
    size: AtomicUsize,
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: H256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        store.insert(tx, data);
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(H256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        store.extend(txs);
        Ok(())
    }

    fn delete(&self, tx: H256) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        store.remove(&tx);
        Ok(())
    }

    fn delete_all(&self, txs: Vec<H256>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        for tx in txs {
            store.remove(&tx);
        }
        Ok(())
    }

    // Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: H256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        let store = self.inner.store.write();
        Ok(store.get(&tx).cloned())
    }

    fn get_all(
        &self,
        txs: Vec<H256>,
    ) -> Result<Vec<(H256, BlobTransactionSidecar)>, BlobStoreError> {
        let mut items = Vec::with_capacity(txs.len());
        let store = self.inner.store.write();
        for tx in txs {
            if let Some(item) = store.get(&tx) {
                items.push((tx, item.clone()));
            }
        }

        Ok(items)
    }

    fn data_size(&self) -> usize {
        self.inner.size.load(std::sync::atomic::Ordering::Relaxed)
    }
}
