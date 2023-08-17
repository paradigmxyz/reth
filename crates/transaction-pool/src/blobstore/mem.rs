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

impl InMemoryBlobStoreInner {

    fn add_size(&self, add: usize) {

    }

    fn sub_size(&self, sub: usize) {

    }

    fn update_size(&self, add: usize, sub: usize) {

    }
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: H256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        let add = data.size();
        let mut store = self.inner.store.write();
        let sub = store.insert(tx, data).map(|rem| rem.size()).unwrap_or_default();
        self.inner.update_size(add, sub);
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(H256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        let mut add = 0;
        for (tx, data) in txs {
            add += data.size();
            store.insert(tx, data);
        }
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

    fn data_size_hint(&self) -> Option<usize> {
        Some(self.inner.size.load(std::sync::atomic::Ordering::Relaxed))
    }
}

#[inline]
fn remove_size(store: &mut HashMap<H256, BlobTransactionSidecar>, tx: &H256) -> usize {
    store.remove(tx).map(|rem| rem.size()).unwrap_or_default()
}

#[inline]
fn insert_size(store: &mut HashMap<H256, BlobTransactionSidecar>, tx: H256, blob: BlobTransactionSidecar) -> usize {
    store.insert(tx).map(|rem| rem.size()).unwrap_or_default()
}