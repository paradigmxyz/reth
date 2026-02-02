use crate::blobstore::{BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobStoreSize};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2},
    eip7594::BlobTransactionSidecarVariant,
};
use alloy_primitives::B256;
use dashmap::DashMap;
use std::sync::Arc;

/// An in-memory blob store.
#[derive(Clone, Debug, Default)]
pub struct InMemoryBlobStore {
    inner: Arc<InMemoryBlobStoreInner>,
}

impl InMemoryBlobStore {
    /// Look up EIP-7594 blobs by their versioned hashes.
    ///
    /// This returns a result vector with the **same length and order** as the input
    /// `versioned_hashes`. Each element is `Some(BlobAndProofV2)` if the blob is available, or
    /// `None` if it is missing or an older sidecar version.
    fn get_by_versioned_hashes_eip7594(
        &self,
        versioned_hashes: &[B256],
    ) -> Vec<Option<BlobAndProofV2>> {
        let mut result = vec![None; versioned_hashes.len()];
        let mut missing_count = result.len();
        for entry in &self.inner.store {
            let blob_sidecar = entry.value();
            if let Some(blob_sidecar) = blob_sidecar.as_eip7594() {
                for (hash_idx, match_result) in
                    blob_sidecar.match_versioned_hashes(versioned_hashes)
                {
                    result[hash_idx] = Some(match_result);
                    missing_count -= 1;
                }
            }

            // Return early if all blobs are found.
            if missing_count == 0 {
                // since versioned_hashes may have duplicates, we double check here
                if result.iter().all(|blob| blob.is_some()) {
                    break;
                }
            }
        }
        result
    }
}

#[derive(Debug, Default)]
struct InMemoryBlobStoreInner {
    /// Storage for all blob data.
    store: DashMap<B256, Arc<BlobTransactionSidecarVariant>>,
    size_tracker: BlobStoreSize,
}

impl PartialEq for InMemoryBlobStore {
    fn eq(&self, other: &Self) -> bool {
        if self.inner.store.len() != other.inner.store.len() {
            return false;
        }
        for entry in &self.inner.store {
            match other.inner.store.get(entry.key()) {
                Some(other_value) if entry.value() == other_value.value() => {}
                _ => return false,
            }
        }
        true
    }
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: B256, data: BlobTransactionSidecarVariant) -> Result<(), BlobStoreError> {
        let add = data.size();
        self.inner.store.insert(tx, Arc::new(data));
        self.inner.size_tracker.add_size(add);
        self.inner.size_tracker.update_len(self.inner.store.len());
        Ok(())
    }

    fn insert_all(
        &self,
        txs: Vec<(B256, BlobTransactionSidecarVariant)>,
    ) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut total_add = 0;
        for (tx, data) in txs {
            let add = data.size();
            self.inner.store.insert(tx, Arc::new(data));
            total_add += add;
        }
        self.inner.size_tracker.add_size(total_add);
        self.inner.size_tracker.update_len(self.inner.store.len());
        Ok(())
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        let sub = self.inner.store.remove(&tx).map(|(_, v)| v.size()).unwrap_or_default();
        self.inner.size_tracker.sub_size(sub);
        self.inner.size_tracker.update_len(self.inner.store.len());
        Ok(())
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut total_sub = 0;
        for tx in txs {
            total_sub += self.inner.store.remove(&tx).map(|(_, v)| v.size()).unwrap_or_default();
        }
        self.inner.size_tracker.sub_size(total_sub);
        self.inner.size_tracker.update_len(self.inner.store.len());
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        BlobStoreCleanupStat::default()
    }

    // Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        Ok(self.inner.store.get(&tx).map(|entry| entry.value().clone()))
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        Ok(self.inner.store.contains_key(&tx))
    }

    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        Ok(txs
            .into_iter()
            .filter_map(|tx| self.inner.store.get(&tx).map(|entry| (tx, entry.value().clone())))
            .collect())
    }

    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(Vec::new());
        }
        txs.into_iter()
            .map(|tx| {
                self.inner
                    .store
                    .get(&tx)
                    .map(|entry| entry.value().clone())
                    .ok_or(BlobStoreError::MissingSidecar(tx))
            })
            .collect()
    }

    fn get_by_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        let mut result = vec![None; versioned_hashes.len()];
        for entry in &self.inner.store {
            let blob_sidecar = entry.value();
            if let Some(blob_sidecar) = blob_sidecar.as_eip4844() {
                for (hash_idx, match_result) in
                    blob_sidecar.match_versioned_hashes(versioned_hashes)
                {
                    result[hash_idx] = Some(match_result);
                }
            }

            // Return early if all blobs are found.
            if result.iter().all(|blob| blob.is_some()) {
                break;
            }
        }
        Ok(result)
    }

    fn get_by_versioned_hashes_v2(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError> {
        let result = self.get_by_versioned_hashes_eip7594(versioned_hashes);
        if result.iter().all(|blob| blob.is_some()) {
            Ok(Some(result.into_iter().map(Option::unwrap).collect()))
        } else {
            Ok(None)
        }
    }

    fn get_by_versioned_hashes_v3(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError> {
        Ok(self.get_by_versioned_hashes_eip7594(versioned_hashes))
    }

    fn data_size_hint(&self) -> Option<usize> {
        Some(self.inner.size_tracker.data_size())
    }

    fn blobs_len(&self) -> usize {
        self.inner.size_tracker.blobs_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::{
        eip4844::{kzg_to_versioned_hash, Blob, BlobAndProofV2, Bytes48},
        eip7594::{
            BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant, CELLS_PER_EXT_BLOB,
        },
    };

    fn eip7594_single_blob_sidecar() -> (BlobTransactionSidecarVariant, B256, BlobAndProofV2) {
        let blob = Blob::default();
        let commitment = Bytes48::default();
        let cell_proofs = vec![Bytes48::default(); CELLS_PER_EXT_BLOB];

        let versioned_hash = kzg_to_versioned_hash(commitment.as_slice());

        let expected =
            BlobAndProofV2 { blob: Box::new(Blob::default()), proofs: cell_proofs.clone() };
        let sidecar = BlobTransactionSidecarEip7594::new(vec![blob], vec![commitment], cell_proofs);

        (BlobTransactionSidecarVariant::Eip7594(sidecar), versioned_hash, expected)
    }

    #[test]
    fn mem_get_blobs_v3_returns_partial_results() {
        let store = InMemoryBlobStore::default();

        let (sidecar, versioned_hash, expected) = eip7594_single_blob_sidecar();
        store.insert(B256::random(), sidecar).unwrap();

        assert_ne!(versioned_hash, B256::ZERO);

        let request = vec![versioned_hash, B256::ZERO];
        let v2 = store.get_by_versioned_hashes_v2(&request).unwrap();
        assert!(v2.is_none(), "v2 must return null if any requested blob is missing");

        let v3 = store.get_by_versioned_hashes_v3(&request).unwrap();
        assert_eq!(v3, vec![Some(expected), None]);
    }
}
