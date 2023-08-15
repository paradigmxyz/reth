use crate::blobstore::{BlobSideCar, BlobStore, BlobStoreError};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    H256,
};
use reth_rlp::Encodable;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};
use tokio::sync::RwLock;

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

#[async_trait::async_trait]
impl BlobStore for InMemoryBlobStore {
    async fn insert(&self, tx: H256, data: BlobSideCar) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write().await;
        store.insert(tx, data);
        Ok(())
    }

    async fn insert_all(&self, txs: Vec<(H256, BlobSideCar)>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write().await;
        store.extend(txs);
        Ok(())
    }

    async fn delete(&self, tx: H256) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write().await;
        store.remove(&tx);
        Ok(())
    }

    async fn delete_all(&self, txs: Vec<H256>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write().await;
        for tx in txs {
            store.remove(&tx);
        }
        Ok(())
    }

    // Retrieves the decoded blob data for the given transaction hash.
    async fn get(&self, tx: H256) -> Result<Option<BlobSideCar>, BlobStoreError> {
        let store = self.inner.store.write().await;
        Ok(store.get(&tx).cloned())
    }

    async fn get_all(&self, txs: Vec<H256>) -> Result<Vec<(H256, BlobSideCar)>, BlobStoreError> {
        let mut items = Vec::new();
        let store = self.inner.store.write().await;
        for tx in txs {
            if let Some(item) = store.get(&tx) {
                items.push((tx, item.clone()));
            }
        }

        Ok(items)
    }

    async fn get_raw(&self, tx: H256) -> Result<Option<Bytes>, BlobStoreError> {
        let item = self.get(tx).await?;
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
