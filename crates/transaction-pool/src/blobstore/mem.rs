use crate::blobstore::{BlobSideCar, BlobStore, BlobStoreError};
use parking_lot::RwLock;
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    H256,
};
use reth_rlp::Encodable;
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
    store: RwLock<HashMap<H256, BlobSideCar>>,
    size: AtomicUsize,
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: H256, data: BlobSideCar) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        store.insert(tx, data);
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(H256, BlobSideCar)>) -> Result<(), BlobStoreError> {
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
    fn get(&self, tx: H256) -> Result<Option<BlobSideCar>, BlobStoreError> {
        let store = self.inner.store.write();
        Ok(store.get(&tx).cloned())
    }

    fn get_all(&self, txs: Vec<H256>) -> Result<Vec<(H256, BlobSideCar)>, BlobStoreError> {
        let mut items = Vec::new();
        let store = self.inner.store.write();
        for tx in txs {
            if let Some(item) = store.get(&tx) {
                items.push((tx, item.clone()));
            }
        }

        Ok(items)
    }

    fn get_raw(&self, tx: H256) -> Result<Option<Bytes>, BlobStoreError> {
        let item = self.get(tx)?;
        Ok(item.map(|item| {
            let mut buf = BytesMut::new();
            item.encode(&mut buf);
            buf.freeze()
        }))
    }

    fn data_size(&self) -> usize {
        self.inner.size.load(std::sync::atomic::Ordering::Relaxed)
    }
}
