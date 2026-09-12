use crate::blobstore::{
    BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobStoreSize, PooledBlobSidecar,
};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{map::B256Map, B256};
use parking_lot::RwLock;
use std::sync::Arc;

/// An in-memory blob store retaining full legacy sidecars or sparse EIP-7594 cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InMemoryBlobStore {
    inner: Arc<InMemoryBlobStoreInner>,
}

impl InMemoryBlobStore {
    fn sidecars(&self) -> Vec<Arc<PooledBlobSidecar>> {
        self.inner.store.read().values().cloned().collect()
    }
}

#[derive(Debug, Default)]
struct InMemoryBlobStoreInner {
    store: RwLock<B256Map<Arc<PooledBlobSidecar>>>,
    size_tracker: BlobStoreSize,
}

impl PartialEq for InMemoryBlobStoreInner {
    fn eq(&self, other: &Self) -> bool {
        self.store.read().eq(&*other.store.read()) && self.size_tracker == other.size_tracker
    }
}

impl BlobStore for InMemoryBlobStore {
    fn transaction_hashes(&self) -> Vec<B256> {
        self.inner.store.read().keys().copied().collect()
    }
    fn transactions(&self) -> Vec<(B256, alloy_primitives::Bytes)> {
        self.inner
            .store
            .read()
            .iter()
            .filter_map(|(hash, sidecar)| sidecar.transaction().map(|body| (*hash, body.clone())))
            .collect()
    }

    fn insert(&self, tx: B256, data: PooledBlobSidecar) -> Result<(), BlobStoreError> {
        self.insert_all(vec![(tx, data)])
    }

    fn insert_all(&self, txs: Vec<(B256, PooledBlobSidecar)>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        for (tx, data) in txs {
            let size = data.size();
            if let Some(previous) = store.insert(tx, Arc::new(data)) {
                self.inner.size_tracker.sub_size(previous.size());
            }
            self.inner.size_tracker.add_size(size);
        }
        self.inner.size_tracker.update_len(store.len());
        Ok(())
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        self.delete_all(vec![tx])
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        for tx in txs {
            if let Some(sidecar) = store.remove(&tx) {
                self.inner.size_tracker.sub_size(sidecar.size());
            }
        }
        self.inner.size_tracker.update_len(store.len());
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        BlobStoreCleanupStat::default()
    }

    fn get_pooled_sidecar(
        &self,
        tx: B256,
    ) -> Result<Option<Arc<PooledBlobSidecar>>, BlobStoreError> {
        Ok(self.inner.store.read().get(&tx).cloned())
    }

    fn get(&self, tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        self.get_pooled_sidecar(tx)?.map(|s| s.full_sidecar()).transpose().map(Option::flatten)
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        Ok(self.inner.store.read().contains_key(&tx))
    }

    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        let mut result = Vec::new();
        for tx in txs {
            if let Some(sidecar) = self.get(tx)? {
                result.push((tx, sidecar));
            }
        }
        Ok(result)
    }

    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        txs.into_iter().map(|tx| self.get(tx)?.ok_or(BlobStoreError::MissingSidecar(tx))).collect()
    }

    fn get_by_versioned_hashes_v1(
        &self,
        hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        let mut result = vec![None; hashes.len()];
        for stored in self.sidecars() {
            if let Some(sidecar) = stored.sidecar().as_eip4844() {
                for (index, value) in sidecar.match_versioned_hashes(hashes) {
                    result[index] = Some(value);
                }
            }
        }
        Ok(result)
    }

    fn get_by_versioned_hashes_v2(
        &self,
        hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError> {
        Ok(self.get_by_versioned_hashes_v3(hashes)?.into_iter().collect())
    }

    fn get_by_versioned_hashes_v3(
        &self,
        hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError> {
        let mut result = vec![None; hashes.len()];
        for stored in self.sidecars() {
            if !stored.versioned_hashes().any(|h| hashes.contains(&h)) {
                continue
            }
            if let Some(full) = stored.full_sidecar()? &&
                let Some(sidecar) = full.as_eip7594()
            {
                for (index, value) in sidecar.match_versioned_hashes(hashes) {
                    result[index] = Some(value);
                }
            }
        }
        Ok(result)
    }

    fn get_by_versioned_hashes_v4(
        &self,
        hashes: &[B256],
        mask: BlobCellMask,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError> {
        let mut result = vec![None; hashes.len()];
        for stored in self.sidecars() {
            for (index, value) in stored.matching_cells(hashes, mask)? {
                super::merge_cell_response(&mut result[index], value);
            }
        }
        Ok(result)
    }

    fn has_versioned_hashes(&self, hashes: &[B256]) -> Result<Vec<bool>, BlobStoreError> {
        let sidecars = self.sidecars();
        Ok(hashes
            .iter()
            .map(|hash| sidecars.iter().any(|s| s.versioned_hashes().any(|h| h == *hash)))
            .collect())
    }

    fn get_cells(&self, tx: B256, mask: BlobCellMask) -> Result<Option<Vec<Cell>>, BlobStoreError> {
        self.get_pooled_sidecar(tx)?.map(|s| s.get_cells(mask)).transpose().map(Option::flatten)
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
    use alloy_consensus::BlobTransactionSidecar;
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

    fn eip4844_single_blob_sidecar() -> (BlobTransactionSidecarVariant, B256) {
        let blob = Blob::default();
        let commitment = Bytes48::from([1u8; 48]);
        let proof = Bytes48::default();
        let versioned_hash = kzg_to_versioned_hash(commitment.as_slice());
        let sidecar = BlobTransactionSidecar {
            blobs: vec![blob],
            commitments: vec![commitment],
            proofs: vec![proof],
        };

        (BlobTransactionSidecarVariant::Eip4844(sidecar), versioned_hash)
    }

    #[test]
    fn mem_has_blobs_returns_ordered_availability() {
        let store = InMemoryBlobStore::default();

        let (eip7594_sidecar, eip7594_hash, _) = eip7594_single_blob_sidecar();
        let (eip4844_sidecar, eip4844_hash) = eip4844_single_blob_sidecar();
        store.insert(B256::random(), eip7594_sidecar.into()).unwrap();
        store.insert(B256::random(), eip4844_sidecar.into()).unwrap();

        let request = vec![eip7594_hash, B256::ZERO, eip4844_hash, eip7594_hash];
        assert_eq!(store.has_versioned_hashes(&request).unwrap(), vec![true, false, true, true]);
    }

    #[test]
    fn mem_get_blobs_v3_returns_partial_results() {
        let store = InMemoryBlobStore::default();

        let (sidecar, versioned_hash, expected) = eip7594_single_blob_sidecar();
        store.insert(B256::random(), sidecar.into()).unwrap();

        assert_ne!(versioned_hash, B256::ZERO);

        let request = vec![versioned_hash, B256::ZERO];
        let v2 = store.get_by_versioned_hashes_v2(&request).unwrap();
        assert!(v2.is_none(), "v2 must return null if any requested blob is missing");

        let v3 = store.get_by_versioned_hashes_v3(&request).unwrap();
        assert_eq!(v3, vec![Some(expected), None]);
    }

    #[test]
    fn mem_get_blobs_v4_returns_requested_cells() {
        let store = InMemoryBlobStore::default();

        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(B256::random(), sidecar.into()).unwrap();

        let indices_bitarray = BlobCellMask::from_bits((1u128 << 0) | (1u128 << 7));
        let request = vec![versioned_hash, B256::ZERO];

        let v4 = store.get_by_versioned_hashes_v4(&request, indices_bitarray).unwrap();
        assert_eq!(v4.len(), request.len());
        assert!(v4[1].is_none());

        let cells_and_proofs = v4[0].as_ref().unwrap();
        assert_eq!(cells_and_proofs.blob_cells.len(), 2);
        assert_eq!(cells_and_proofs.proofs.len(), 2);
        assert!(cells_and_proofs.blob_cells.iter().all(Option::is_some));
        assert_eq!(cells_and_proofs.proofs, vec![Some(Bytes48::default()); 2]);
    }

    #[test]
    fn mem_get_cells_returns_requested_cells() {
        let store = InMemoryBlobStore::default();

        let tx_hash = B256::random();
        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(tx_hash, sidecar.into()).unwrap();

        let indices_bitarray = BlobCellMask::from_bits((1u128 << 0) | (1u128 << 7));
        let expected = store
            .get_by_versioned_hashes_v4(&[versioned_hash], indices_bitarray)
            .unwrap()
            .pop()
            .unwrap()
            .unwrap()
            .blob_cells
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .unwrap();

        assert_eq!(store.get_cells(tx_hash, indices_bitarray).unwrap(), Some(expected));
    }
}
