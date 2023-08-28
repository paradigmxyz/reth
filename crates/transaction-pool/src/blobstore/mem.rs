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
    data_size: AtomicUsize,
    num_blobs: AtomicUsize,
}

impl InMemoryBlobStoreInner {
    fn add_size(&self, add: usize) {
        self.data_size.fetch_add(add, std::sync::atomic::Ordering::Relaxed);
    }

    fn sub_size(&self, sub: usize) {
        self.data_size.fetch_sub(sub, std::sync::atomic::Ordering::Relaxed);
    }

    fn update_size(&self, add: usize, sub: usize) {
        if add > sub {
            self.add_size(add - sub);
        } else {
            self.sub_size(sub - add);
        }
    }

    fn update_len(&self, len: usize) {
        self.num_blobs.store(len, std::sync::atomic::Ordering::Relaxed);
    }
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: H256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        let (add, sub) = insert_size(&mut store, tx, data);
        self.inner.update_size(add, sub);
        self.inner.update_len(store.len());
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(H256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut store = self.inner.store.write();
        let mut total_add = 0;
        let mut total_sub = 0;
        for (tx, data) in txs {
            let (add, sub) = insert_size(&mut store, tx, data);
            total_add += add;
            total_sub += sub;
        }
        self.inner.update_size(total_add, total_sub);
        self.inner.update_len(store.len());
        Ok(())
    }

    fn delete(&self, tx: H256) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        let sub = remove_size(&mut store, &tx);
        self.inner.sub_size(sub);
        self.inner.update_len(store.len());
        Ok(())
    }

    fn delete_all(&self, txs: Vec<H256>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut store = self.inner.store.write();
        let mut total_sub = 0;
        for tx in txs {
            total_sub += remove_size(&mut store, &tx);
        }
        self.inner.sub_size(total_sub);
        self.inner.update_len(store.len());
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
        Some(self.inner.data_size.load(std::sync::atomic::Ordering::Relaxed))
    }

    fn blobs_len(&self) -> usize {
        self.inner.num_blobs.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Removes the given blob from the store and returns the size of the blob that was removed.
#[inline]
fn remove_size(store: &mut HashMap<H256, BlobTransactionSidecar>, tx: &H256) -> usize {
    store.remove(tx).map(|rem| rem.size()).unwrap_or_default()
}

/// Inserts the given blob into the store and returns the size of the blob that was (added,removed)
#[inline]
fn insert_size(
    store: &mut HashMap<H256, BlobTransactionSidecar>,
    tx: H256,
    blob: BlobTransactionSidecar,
) -> (usize, usize) {
    let add = blob.size();
    let sub = store.insert(tx, blob).map(|rem| rem.size()).unwrap_or_default();
    (add, sub)
}
