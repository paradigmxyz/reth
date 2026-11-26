use crate::blobstore::{
    cells::{merge_cells, missing_hashes},
    BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobStoreSize, BlobTxCellSidecar,
    PooledBlobSidecar, RebuiltSidecars,
};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use parking_lot::RwLock;
use std::sync::Arc;

/// An in-memory blob store.
#[derive(Clone, Debug, Default, PartialEq)]
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
        for blob_sidecar in self.inner.store.read().values() {
            if let Some(blob_sidecar) = blob_sidecar.as_eip7594() {
                for (hash_idx, match_result) in
                    blob_sidecar.match_versioned_hashes(versioned_hashes)
                {
                    let slot = &mut result[hash_idx];
                    if slot.is_none() {
                        missing_count -= 1;
                    }
                    *slot = Some(match_result);
                }
            }

            // Return early if all blobs are found.
            if missing_count == 0 {
                // since versioned_hashes may have duplicates, we double check here
                if result.iter().all(|blob| blob.is_some()) {
                    return result
                }
            }
        }
        let mut missing = missing_hashes(versioned_hashes, &result);
        for (tx, cells) in self.inner.cells_matching(&missing) {
            // Sidecars sharing a blob are rebuilt only while it is missing.
            if cells.versioned_hashes().any(|hash| missing.contains(&hash)) &&
                let Some(sidecar) = self.inner.rebuilt.get_or_rebuild(tx, &cells) &&
                let Some(sidecar) = sidecar.as_eip7594()
            {
                for (hash_idx, match_result) in sidecar.match_versioned_hashes(versioned_hashes) {
                    missing.remove(&versioned_hashes[hash_idx]);
                    result[hash_idx].get_or_insert(match_result);
                }
            }
        }
        result
    }

    /// Look up EIP-7594 blob cells by their versioned hashes.
    fn get_by_versioned_hashes_cells_eip7594(
        &self,
        versioned_hashes: &[B256],
        cell_mask: BlobCellMask,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError> {
        let mut result = vec![None; versioned_hashes.len()];
        let mut missing_count = result.len();
        let blob_sidecars = self.inner.store.read().values().cloned().collect::<Vec<_>>();
        for blob_sidecar in blob_sidecars {
            if let Some(blob_sidecar) = blob_sidecar.as_eip7594() {
                for (hash_idx, match_result) in blob_sidecar
                    .match_versioned_hashes_cells(versioned_hashes, cell_mask)
                    .map_err(|err| BlobStoreError::Other(Box::new(err)))?
                {
                    let slot = &mut result[hash_idx];
                    if slot.is_none() {
                        missing_count -= 1;
                    }
                    *slot = Some(match_result);
                }
            }

            if missing_count == 0 && result.iter().all(Option::is_some) {
                return Ok(result)
            }
        }
        for (_, cells) in self.inner.cells_matching(&missing_hashes(versioned_hashes, &result)) {
            for (hash_idx, match_result) in
                cells.match_versioned_hashes_cells(versioned_hashes, cell_mask)
            {
                merge_cells(&mut result[hash_idx], match_result);
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Default)]
struct InMemoryBlobStoreInner {
    /// Storage for all blob data.
    store: RwLock<B256Map<Arc<BlobTransactionSidecarVariant>>>,
    // Sidecars that keep only some cells, for transactions not in `store`. Locked after `store`
    // when both are held.
    cells: RwLock<B256Map<Arc<BlobTxCellSidecar>>>,
    // Sidecars rebuilt from `cells`.
    rebuilt: RebuiltSidecars,
    size_tracker: BlobStoreSize,
}

impl InMemoryBlobStoreInner {
    // Returns the sidecar of `tx`, rebuilding it from cells if needed.
    fn get(&self, tx: &B256) -> Option<Arc<BlobTransactionSidecarVariant>> {
        let cells = {
            // Held across both lookups, so a move from cells to full is not missed.
            let store = self.store.read();
            if let Some(sidecar) = store.get(tx) {
                return Some(Arc::clone(sidecar))
            }
            self.cells.read().get(tx).cloned()?
        };
        self.rebuilt.get_or_rebuild(*tx, &cells)
    }

    // Returns the cell sidecars with a blob in `hashes`.
    fn cells_matching(&self, hashes: &B256Set) -> Vec<(B256, Arc<BlobTxCellSidecar>)> {
        self.cells
            .read()
            .iter()
            .filter(|(_, cells)| cells.versioned_hashes().any(|hash| hashes.contains(&hash)))
            .map(|(tx, cells)| (*tx, Arc::clone(cells)))
            .collect()
    }

    // Removes the cell sidecar of `tx` and returns its size.
    fn remove_cells(&self, cells: &mut B256Map<Arc<BlobTxCellSidecar>>, tx: &B256) -> usize {
        self.rebuilt.remove(tx);
        cells.remove(tx).map(|cells| cells.size()).unwrap_or_default()
    }
}

impl PartialEq for InMemoryBlobStoreInner {
    fn eq(&self, other: &Self) -> bool {
        self.store.read().eq(&*other.store.read()) && self.cells.read().eq(&*other.cells.read())
    }
}

impl BlobStore for InMemoryBlobStore {
    fn insert(&self, tx: B256, data: PooledBlobSidecar) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        let mut cells = self.inner.cells.write();
        let sub = self.inner.remove_cells(&mut cells, &tx);
        self.inner.size_tracker.sub_size(sub);
        self.inner.size_tracker.add_size(insert_size(&mut store, tx, data.into_sidecar()));
        self.inner.size_tracker.update_len(store.len() + cells.len());
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(B256, PooledBlobSidecar)>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut store = self.inner.store.write();
        let mut cells = self.inner.cells.write();
        let mut total_add = 0;
        let mut total_sub = 0;
        for (tx, data) in txs {
            total_sub += self.inner.remove_cells(&mut cells, &tx);
            let add = insert_size(&mut store, tx, data.into_sidecar());
            total_add += add;
        }
        self.inner.size_tracker.sub_size(total_sub);
        self.inner.size_tracker.add_size(total_add);
        self.inner.size_tracker.update_len(store.len() + cells.len());
        Ok(())
    }

    fn insert_cells(&self, tx: B256, data: BlobTxCellSidecar) -> Result<(), BlobStoreError> {
        let store = self.inner.store.read();
        let mut cells = self.inner.cells.write();
        if !store.contains_key(&tx) && !cells.contains_key(&tx) {
            self.inner.size_tracker.add_size(data.size());
            cells.insert(tx, Arc::new(data));
            self.inner.size_tracker.update_len(store.len() + cells.len());
        }
        Ok(())
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        let mut store = self.inner.store.write();
        let mut cells = self.inner.cells.write();
        let sub = remove_size(&mut store, &tx) + self.inner.remove_cells(&mut cells, &tx);
        self.inner.size_tracker.sub_size(sub);
        self.inner.size_tracker.update_len(store.len() + cells.len());
        Ok(())
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let mut store = self.inner.store.write();
        let mut cells = self.inner.cells.write();
        let mut total_sub = 0;
        for tx in txs {
            total_sub += remove_size(&mut store, &tx) + self.inner.remove_cells(&mut cells, &tx);
        }
        self.inner.size_tracker.sub_size(total_sub);
        self.inner.size_tracker.update_len(store.len() + cells.len());
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        BlobStoreCleanupStat::default()
    }

    // Retrieves the decoded blob data for the given transaction hash.
    fn get(&self, tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        Ok(self.inner.get(&tx))
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        Ok(self.inner.store.read().contains_key(&tx) || self.inner.cells.read().contains_key(&tx))
    }

    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        Ok(txs.into_iter().filter_map(|tx| self.inner.get(&tx).map(|item| (tx, item))).collect())
    }

    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(Vec::new());
        }
        txs.into_iter()
            .map(|tx| self.inner.get(&tx).ok_or(BlobStoreError::MissingSidecar(tx)))
            .collect()
    }

    fn get_by_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        let mut result = vec![None; versioned_hashes.len()];
        for blob_sidecar in self.inner.store.read().values() {
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

    fn get_by_versioned_hashes_v4(
        &self,
        versioned_hashes: &[B256],
        cell_mask: BlobCellMask,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError> {
        self.get_by_versioned_hashes_cells_eip7594(versioned_hashes, cell_mask)
    }

    fn has_versioned_hashes(&self, versioned_hashes: &[B256]) -> Result<Vec<bool>, BlobStoreError> {
        let mut result = vec![false; versioned_hashes.len()];
        for blob_sidecar in self.inner.store.read().values() {
            for available_hash in blob_sidecar.versioned_hashes() {
                for (idx, requested_hash) in versioned_hashes.iter().enumerate() {
                    if !result[idx] && *requested_hash == available_hash {
                        result[idx] = true;
                    }
                }
            }

            if result.iter().all(|available| *available) {
                return Ok(result)
            }
        }
        // Only blobs that can be rebuilt from their cells count as available.
        for cells in self.inner.cells.read().values().filter(|cells| cells.is_recoverable()) {
            for available_hash in cells.versioned_hashes() {
                for (idx, requested_hash) in versioned_hashes.iter().enumerate() {
                    if *requested_hash == available_hash {
                        result[idx] = true;
                    }
                }
            }
        }
        Ok(result)
    }

    fn get_cells(
        &self,
        tx: B256,
        cell_mask: BlobCellMask,
    ) -> Result<Option<Vec<Cell>>, BlobStoreError> {
        let Some(sidecar) = self.inner.store.read().get(&tx).cloned() else {
            return Ok(self.inner.cells.read().get(&tx).and_then(|cells| cells.get_cells(cell_mask)))
        };

        let Some(sidecar) = sidecar.as_eip7594() else {
            return Ok(None);
        };

        sidecar
            .compute_matching_cells(cell_mask)
            .map(Some)
            .map_err(|err| BlobStoreError::Other(Box::new(err)))
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
fn remove_size(store: &mut B256Map<Arc<BlobTransactionSidecarVariant>>, tx: &B256) -> usize {
    store.remove(tx).map(|rem| rem.size()).unwrap_or_default()
}

/// Inserts the given blob into the store and returns the size of the blob that was added.
///
/// We don't need to handle the size updates for replacements because transactions are unique.
#[inline]
fn insert_size(
    store: &mut B256Map<Arc<BlobTransactionSidecarVariant>>,
    tx: B256,
    blob: BlobTransactionSidecarVariant,
) -> usize {
    let add = blob.size();
    store.insert(tx, Arc::new(blob));
    add
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobstore::cells::tests::{fixture, subset};
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
        let commitment = Bytes48::repeat_byte(1u8);
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

        let cell_mask = BlobCellMask::from_bits((1u128 << 0) | (1u128 << 7));
        let request = vec![versioned_hash, B256::ZERO];

        let v4 = store.get_by_versioned_hashes_v4(&request, cell_mask).unwrap();
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

        let cell_mask = BlobCellMask::from_bits((1u128 << 0) | (1u128 << 7));
        let expected = store
            .get_by_versioned_hashes_v4(&[versioned_hash], cell_mask)
            .unwrap()
            .pop()
            .unwrap()
            .unwrap()
            .blob_cells
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .unwrap();

        assert_eq!(store.get_cells(tx_hash, cell_mask).unwrap(), Some(expected));
    }

    #[test]
    fn mem_get_blobs_v3_rebuilds_a_shared_blob_once() {
        let store = InMemoryBlobStore::default();
        let (_, all) = fixture();
        let hash = all.versioned_hashes().next().unwrap();
        let half = subset(BlobCellMask::from_bits(u128::MAX / 3));
        store.insert_cells(B256::random(), half.clone()).unwrap();
        store.insert_cells(B256::random(), half).unwrap();

        assert!(store.get_by_versioned_hashes_v3(&[hash]).unwrap()[0].is_some());
        assert_eq!(store.inner.rebuilt.cache.lock().sidecars.len(), 1);
    }
}
