use crate::blobstore::{
    BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobStoreSize, BlobTransactionSidecar,
};
use alloy_eips::eip4844::BlobAndProofV1;
use alloy_primitives::B256;
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc};

/// An in-memory blob store.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InMemoryBlobStore {
    inner: Arc<InMemoryBlobStoreInner>,
}

#[derive(Debug, Default)]
struct InMemoryBlobStoreInner {
    /// Storage for all blob data.
    store: RwLock<HashMap<B256, BlobTransactionSidecar>>,
    size_tracker: BlobStoreSize,
}

impl PartialEq for InMemoryBlobStoreInner {
    fn eq(&self, other: &Self) -> bool {
        self.store.read().eq(&other.store.read())
    }
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: B256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        self.inner.size_tracker.add_size(insert_size(&mut store, tx, data));
        self.inner.size_tracker.update_len(store.len());
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(B256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut store = self.inner.store.write();
        let mut total_add = 0;
        for (tx, data) in txs {
            let add = insert_size(&mut store, tx, data);
            total_add += add;
        }
        self.inner.size_tracker.add_size(total_add);
        self.inner.size_tracker.update_len(store.len());
        Ok(())
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        let sub = remove_size(&mut store, &tx);
        self.inner.size_tracker.sub_size(sub);
        self.inner.size_tracker.update_len(store.len());
        Ok(())
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut store = self.inner.store.write();
        let mut total_sub = 0;
        for tx in txs {
            total_sub += remove_size(&mut store, &tx);
        }
        self.inner.size_tracker.sub_size(total_sub);
        self.inner.size_tracker.update_len(store.len());
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        BlobStoreCleanupStat::default()
    }

    // Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        let store = self.inner.store.read();
        Ok(store.get(&tx).cloned())
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        let store = self.inner.store.read();
        Ok(store.contains_key(&tx))
    }

    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError> {
        let mut items = Vec::with_capacity(txs.len());
        let store = self.inner.store.read();
        for tx in txs {
            if let Some(item) = store.get(&tx) {
                items.push((tx, item.clone()));
            }
        }

        Ok(items)
    }

    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        let mut items = Vec::with_capacity(txs.len());
        let store = self.inner.store.read();
        for tx in txs {
            if let Some(item) = store.get(&tx) {
                items.push(item.clone());
            } else {
                return Err(BlobStoreError::MissingSidecar(tx))
            }
        }

        Ok(items)
    }

    fn get_by_versioned_hashes(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        let mut result = vec![None; versioned_hashes.len()];
        for (_tx_hash, blob_sidecar) in self.inner.store.read().iter() {
            for (i, blob_versioned_hash) in blob_sidecar.versioned_hashes().enumerate() {
                for (j, target_versioned_hash) in versioned_hashes.iter().enumerate() {
                    if blob_versioned_hash == *target_versioned_hash {
                        result[j].get_or_insert_with(|| BlobAndProofV1 {
                            blob: Box::new(blob_sidecar.blobs[i]),
                            proof: blob_sidecar.proofs[i],
                        });
                    }
                }
            }

            // Return early if all blobs are found.
            if result.iter().all(|blob| blob.is_some()) {
                break;
            }
        }
        Ok(result)
    }

    fn data_size_hint(&self) -> Option<usize> {
        Some(self.inner.size_tracker.data_size())
    }

    fn blobs_len(&self) -> usize {
        self.inner.size_tracker.blobs_len()
    }
}

/// Removes the given blob from the store and returns the size of the blob that was removed.
#[inline]
fn remove_size(store: &mut HashMap<B256, BlobTransactionSidecar>, tx: &B256) -> usize {
    store.remove(tx).map(|rem| rem.size()).unwrap_or_default()
}

/// Inserts the given blob into the store and returns the size of the blob that was added.
///
/// We don't need to handle the size updates for replacements because transactions are unique.
#[inline]
fn insert_size(
    store: &mut HashMap<B256, BlobTransactionSidecar>,
    tx: B256,
    blob: BlobTransactionSidecar,
) -> usize {
    let add = blob.size();
    store.insert(tx, blob);
    add
}
