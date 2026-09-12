//! Persistent full and cell-based blob sidecars.

use crate::blobstore::{
    BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobStoreSize, PooledBlobSidecar,
};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    TxHash, B256,
};
use parking_lot::{Mutex, RwLock};
use schnellru::{Limiter, LruMap};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};
use tracing::debug;

/// Maximum number of sidecars retained in the memory cache by default.
pub const DEFAULT_MAX_CACHED_BLOBS: u32 = 100;

/// A disk-backed blob store with a bounded decoded-sidecar cache.
///
/// Cell-backed entries are served without reconstructing blobs. Full blobs are reconstructed only
/// for legacy consumers, then memoized with the cached sidecar. Deletion is deferred to cleanup.
#[derive(Clone, Debug)]
pub struct DiskFileBlobStore {
    inner: Arc<DiskFileBlobStoreInner>,
}

impl DiskFileBlobStore {
    /// Opens the store, optionally retaining and re-indexing existing sidecars.
    pub fn open(
        blob_dir: impl Into<PathBuf>,
        opts: DiskFileBlobStoreConfig,
    ) -> Result<Self, DiskFileBlobStoreError> {
        let blob_dir = blob_dir.into();
        if opts.open == OpenDiskFileBlobStore::Clear {
            match fs::remove_dir_all(&blob_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(DiskFileBlobStoreError::Open(blob_dir, e)),
            }
        }
        reth_fs_util::create_dir_all(&blob_dir)
            .map_err(|e| DiskFileBlobStoreError::Open(blob_dir.clone(), io::Error::other(e)))?;
        let store = Self {
            inner: Arc::new(DiskFileBlobStoreInner {
                blob_dir: blob_dir.clone(),
                blob_cache: Mutex::new(LruMap::new(SidecarCacheLimit {
                    max_entries: opts.max_cached_entries,
                    bytes: 0,
                })),
                size_tracker: Default::default(),
                file_lock: Default::default(),
                txs_to_delete: Default::default(),
                versioned_hashes_to_txhash: Default::default(),
                transactions: Default::default(),
                pinned: Default::default(),
                cell_mode: Default::default(),
            }),
        };
        if opts.open == OpenDiskFileBlobStore::ReIndex {
            for entry in reth_fs_util::read_dir(&blob_dir)
                .map_err(|e| DiskFileBlobStoreError::Open(blob_dir.clone(), io::Error::other(e)))?
            {
                let entry = entry.map_err(|e| DiskFileBlobStoreError::Open(blob_dir.clone(), e))?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
                let Ok(tx) = name.parse::<B256>() else { continue };
                let data = reth_fs_util::read(entry.path())
                    .map_err(|e| DiskFileBlobStoreError::Open(entry.path(), io::Error::other(e)))?;
                match PooledBlobSidecar::decode_stored(&data) {
                    Ok(sidecar) => {
                        store.inner.index(tx, &sidecar);
                        store.inner.size_tracker.add_size(data.len());
                        store.inner.size_tracker.inc_len(1);
                    }
                    Err(err) => {
                        debug!(target: "txpool::blob", %tx, %err, "Discarding corrupt blob sidecar");
                        reth_fs_util::remove_file(entry.path()).map_err(|e| {
                            DiskFileBlobStoreError::Open(entry.path(), io::Error::other(e))
                        })?;
                    }
                }
            }
        }
        Ok(store)
    }

    #[cfg(test)]
    fn is_cached(&self, tx: &B256) -> bool {
        self.inner.blob_cache.lock().get(tx).is_some()
    }
    #[cfg(test)]
    fn clear_cache(&self) {
        self.inner.blob_cache.lock().clear();
    }

    fn candidates(&self, hashes: &[B256]) -> Result<Vec<Arc<PooledBlobSidecar>>, BlobStoreError> {
        let txs: B256Set = {
            let index = self.inner.versioned_hashes_to_txhash.read();
            hashes.iter().filter_map(|hash| index.get(hash)).flatten().copied().collect()
        };
        let mut result = Vec::with_capacity(txs.len());
        for tx in txs {
            if let Some(sidecar) = self.get_pooled_sidecar(tx)? {
                result.push(sidecar);
            }
        }
        Ok(result)
    }
}

impl BlobStore for DiskFileBlobStore {
    fn set_cell_mode(&self) {
        self.inner.cell_mode.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn warm_transactions(&self, hashes: &[B256]) {
        for hash in hashes {
            if let Ok(Some(sidecar)) = self.get_pooled_sidecar(*hash) &&
                !self.inner.cell_mode.load(std::sync::atomic::Ordering::Relaxed)
            {
                let _ = sidecar.full_sidecar();
            }
        }
    }

    fn warm_versioned_hashes(&self, hashes: &[B256]) {
        let txs: B256Set = self
            .inner
            .versioned_hashes_to_txhash
            .read()
            .iter()
            .filter(|(hash, _)| hashes.contains(hash))
            .flat_map(|(_, txs)| txs.iter().copied())
            .collect();
        for tx in txs {
            let Ok(Some(sidecar)) = self.get_pooled_sidecar(tx) else { continue };
            if !self.inner.cell_mode.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = sidecar.full_sidecar();
            }
            let _lock = self.inner.file_lock.read();
            if !self
                .inner
                .blob_cache
                .lock()
                .peek(&tx)
                .is_some_and(|current| Arc::ptr_eq(current, &sidecar))
            {
                continue
            }
            let now = std::time::Instant::now();
            let mut pinned = self.inner.pinned.lock();
            pinned.retain(|_, (expires, _)| *expires > now);
            let used: usize = pinned
                .iter()
                .filter(|(hash, _)| **hash != tx)
                .map(|(_, (_, sidecar))| SidecarCacheLimit::cost(sidecar))
                .sum();
            if used + SidecarCacheLimit::cost(&sidecar) <= SidecarCacheLimit::MAX_BYTES {
                pinned.insert(tx, (now + std::time::Duration::from_secs(1), sidecar));
            }
        }
    }

    fn transaction_hashes(&self) -> Vec<B256> {
        self.inner
            .versioned_hashes_to_txhash
            .read()
            .values()
            .flatten()
            .copied()
            .collect::<B256Set>()
            .into_iter()
            .collect()
    }

    fn transactions(&self) -> Vec<(B256, alloy_primitives::Bytes)> {
        self.inner.transactions.read().iter().map(|(hash, body)| (*hash, body.clone())).collect()
    }

    fn insert(&self, tx: B256, data: PooledBlobSidecar) -> Result<(), BlobStoreError> {
        let encoded = data.encode_stored();
        let _lock = self.inner.file_lock.write();
        let path = self.inner.blob_disk_file(tx);
        let previous_size = fs::metadata(&path).ok().map(|m| m.len() as usize);
        reth_fs_util::atomic_write_file(&path, |file| file.write_all(&encoded))
            .map_err(|e| BlobStoreError::Other(Box::new(e)))?;
        self.inner.index(tx, &data);
        self.inner.pinned.lock().remove(&tx);
        self.inner.blob_cache.lock().insert(tx, Arc::new(data));
        self.inner.txs_to_delete.write().remove(&tx);
        if let Some(size) = previous_size {
            self.inner.size_tracker.sub_size(size);
        } else {
            self.inner.size_tracker.inc_len(1);
        }
        self.inner.size_tracker.add_size(encoded.len());
        Ok(())
    }

    fn insert_all(&self, txs: Vec<(B256, PooledBlobSidecar)>) -> Result<(), BlobStoreError> {
        for (tx, sidecar) in txs {
            self.insert(tx, sidecar)?;
        }
        Ok(())
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        let _lock = self.inner.file_lock.read();
        if self.inner.blob_disk_file(tx).is_file() {
            self.inner.txs_to_delete.write().insert(tx);
        }
        Ok(())
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        for tx in txs {
            self.delete(tx)?;
        }
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        let _lock = self.inner.file_lock.write();
        let txs = std::mem::take(&mut *self.inner.txs_to_delete.write());
        let mut result = BlobStoreCleanupStat::default();
        for tx in txs {
            let path = self.inner.blob_disk_file(tx);
            let size = fs::metadata(&path).ok().map(|m| m.len() as usize);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    debug!(target: "txpool::blob", %tx, %err, "Failed to delete blob sidecar");
                    self.inner.txs_to_delete.write().insert(tx);
                    result.delete_failed += 1;
                    continue
                }
            }
            self.inner.pinned.lock().remove(&tx);
            self.inner.blob_cache.lock().remove(&tx);
            self.inner.transactions.write().remove(&tx);
            self.inner.versioned_hashes_to_txhash.write().retain(|_, txs| {
                txs.remove(&tx);
                !txs.is_empty()
            });
            if let Some(size) = size {
                self.inner.size_tracker.sub_size(size);
                self.inner.size_tracker.sub_len(1);
            }
            result.delete_succeed += 1;
        }
        result
    }

    fn get_pooled_sidecar(
        &self,
        tx: B256,
    ) -> Result<Option<Arc<PooledBlobSidecar>>, BlobStoreError> {
        let _lock = self.inner.file_lock.read();
        if let Some((expires, sidecar)) = self.inner.pinned.lock().get(&tx) &&
            *expires > std::time::Instant::now()
        {
            return Ok(Some(sidecar.clone()))
        }
        if let Some(sidecar) = self.inner.blob_cache.lock().get(&tx).cloned() {
            return Ok(Some(sidecar))
        }
        let path = self.inner.blob_disk_file(tx);
        let encoded = match reth_fs_util::read(&path) {
            Ok(data) => data,
            Err(reth_fs_util::FsPathError::Read { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(None)
            }
            Err(err) => return Err(BlobStoreError::Other(Box::new(err))),
        };
        let sidecar = Arc::new(PooledBlobSidecar::decode_stored(&encoded)?);
        self.inner.blob_cache.lock().insert(tx, sidecar.clone());
        Ok(Some(sidecar))
    }

    fn get(&self, tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        self.get_pooled_sidecar(tx)?.map(|s| s.full_sidecar()).transpose().map(Option::flatten)
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        let _lock = self.inner.file_lock.read();
        Ok(self.inner.blob_disk_file(tx).is_file())
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
        for stored in self.candidates(hashes)? {
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
        for stored in self.candidates(hashes)? {
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
        for stored in self.candidates(hashes)? {
            for (index, value) in stored.matching_cells(hashes, mask)? {
                super::merge_cell_response(&mut result[index], value);
            }
        }
        Ok(result)
    }

    fn has_versioned_hashes(&self, hashes: &[B256]) -> Result<Vec<bool>, BlobStoreError> {
        let index = self.inner.versioned_hashes_to_txhash.read();
        Ok(hashes.iter().map(|h| index.get(h).is_some_and(|txs| !txs.is_empty())).collect())
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

#[derive(Debug)]
struct DiskFileBlobStoreInner {
    cell_mode: std::sync::atomic::AtomicBool,
    blob_dir: PathBuf,
    blob_cache: Mutex<LruMap<TxHash, Arc<PooledBlobSidecar>, SidecarCacheLimit>>,
    size_tracker: BlobStoreSize,
    file_lock: RwLock<()>,
    txs_to_delete: RwLock<B256Set>,
    versioned_hashes_to_txhash: RwLock<B256Map<B256Set>>,
    transactions: RwLock<B256Map<alloy_primitives::Bytes>>,
    pinned: Mutex<B256Map<(std::time::Instant, Arc<PooledBlobSidecar>)>>,
}

impl DiskFileBlobStoreInner {
    fn blob_disk_file(&self, tx: B256) -> PathBuf {
        self.blob_dir.join(format!("{tx:x}"))
    }
    fn index(&self, tx: B256, sidecar: &PooledBlobSidecar) {
        if let Some(body) = sidecar.transaction() {
            self.transactions.write().insert(tx, body.clone());
        }
        let mut index = self.versioned_hashes_to_txhash.write();
        for hash in sidecar.versioned_hashes() {
            index.entry(hash).or_default().insert(tx);
        }
    }
}

/// Errors that can occur when interacting with a disk file blob store.
#[derive(Debug, thiserror::Error)]
pub enum DiskFileBlobStoreError {
    /// Thrown during [`DiskFileBlobStore::open`] if the blob store directory cannot be opened.
    #[error("failed to open blobstore at {0}: {1}")]
    /// Indicates a failure to open the blob store directory.
    Open(PathBuf, io::Error),
    /// Failure while reading a blob file.
    #[error("[{0}] failed to read blob file at {1}: {2}")]
    /// Indicates a failure while reading a blob file.
    ReadFile(TxHash, PathBuf, io::Error),
    /// Failure while writing a blob file.
    #[error("[{0}] failed to write blob file at {1}: {2}")]
    /// Indicates a failure while writing a blob file.
    WriteFile(TxHash, PathBuf, io::Error),
    /// Failure while deleting a blob file.
    #[error("[{0}] failed to delete blob file at {1}: {2}")]
    /// Indicates a failure while deleting a blob file.
    DeleteFile(TxHash, PathBuf, io::Error),
}

impl From<DiskFileBlobStoreError> for BlobStoreError {
    fn from(value: DiskFileBlobStoreError) -> Self {
        Self::Other(Box::new(value))
    }
}

/// Configuration for a disk file blob store.
#[derive(Debug, Clone)]
pub struct DiskFileBlobStoreConfig {
    /// The maximum number of blobs to keep in the in memory blob cache.
    pub max_cached_entries: u32,
    /// How to open the blob store.
    pub open: OpenDiskFileBlobStore,
}

impl Default for DiskFileBlobStoreConfig {
    fn default() -> Self {
        Self { max_cached_entries: DEFAULT_MAX_CACHED_BLOBS, open: Default::default() }
    }
}

impl DiskFileBlobStoreConfig {
    /// Set maximum number of blobs to keep in the in memory blob cache.
    pub const fn with_max_cached_entries(mut self, max_cached_entries: u32) -> Self {
        self.max_cached_entries = max_cached_entries;
        self
    }
}

/// How to open a disk file blob store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenDiskFileBlobStore {
    /// Clear everything in the blob store.
    #[default]
    Clear,
    /// Keep the existing blob store and index
    ReIndex,
}

#[cfg(test)]
mod tests {
    use alloy_consensus::BlobTransactionSidecar;
    use alloy_eips::{
        eip4844::{kzg_to_versioned_hash, Blob, BlobAndProofV2, Bytes48},
        eip7594::{
            BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant, CELLS_PER_EXT_BLOB,
        },
    };

    use super::*;
    use std::sync::atomic::Ordering;

    fn tmp_store() -> (DiskFileBlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskFileBlobStore::open(dir.path(), Default::default()).unwrap();
        (store, dir)
    }

    fn rng_blobs(num: usize) -> Vec<(TxHash, BlobTransactionSidecarVariant)> {
        let mut rng = rand::rng();
        (0..num)
            .map(|_| {
                let tx = TxHash::random_with(&mut rng);
                let blob = BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar {
                    blobs: vec![],
                    commitments: vec![],
                    proofs: vec![],
                });
                (tx, blob)
            })
            .collect()
    }

    fn wrapped_blobs(
        blobs: Vec<(TxHash, BlobTransactionSidecarVariant)>,
    ) -> Vec<(TxHash, PooledBlobSidecar)> {
        blobs.into_iter().map(|(tx, blob)| (tx, blob.into())).collect()
    }

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
    fn disk_insert_all_get_all() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(10);
        let all_hashes = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs.clone())).unwrap();

        // all cached
        for (tx, blob) in &blobs {
            assert!(store.is_cached(tx));
            let b = store.get(*tx).unwrap().map(Arc::unwrap_or_clone).unwrap();
            assert_eq!(b, *blob);
        }

        let all = store.get_all(all_hashes.clone()).unwrap();
        for (tx, blob) in all {
            assert!(blobs.contains(&(tx, Arc::unwrap_or_clone(blob))), "missing blob {tx:?}");
        }

        assert!(store.contains(all_hashes[0]).unwrap());
        store.delete_all(all_hashes.clone()).unwrap();
        assert!(store.inner.txs_to_delete.read().contains(&all_hashes[0]));
        store.clear_cache();
        store.cleanup();

        assert!(store.get(blobs[0].0).unwrap().is_none());

        let all = store.get_all(all_hashes.clone()).unwrap();
        assert!(all.is_empty());

        assert!(!store.contains(all_hashes[0]).unwrap());
        assert!(store.get_exact(all_hashes).is_err());

        assert_eq!(store.data_size_hint(), Some(0));
        assert_eq!(store.inner.size_tracker.num_blobs.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn disk_insert_and_retrieve() {
        let (store, _dir) = tmp_store();

        let (tx, blob) = rng_blobs(1).into_iter().next().unwrap();
        store.insert(tx, blob.clone().into()).unwrap();

        assert!(store.is_cached(&tx));
        let retrieved_blob = store.get(tx).unwrap().map(Arc::unwrap_or_clone).unwrap();
        assert_eq!(retrieved_blob, blob);
    }

    #[test]
    fn disk_delete_blob() {
        let (store, _dir) = tmp_store();

        let (tx, blob) = rng_blobs(1).into_iter().next().unwrap();
        store.insert(tx, blob.into()).unwrap();
        assert!(store.is_cached(&tx));

        store.delete(tx).unwrap();
        assert!(store.inner.txs_to_delete.read().contains(&tx));
        store.cleanup();

        let result = store.get(tx).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn disk_insert_all_and_delete_all() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(5);
        let txs = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs.clone())).unwrap();

        for (tx, _) in &blobs {
            assert!(store.is_cached(tx));
        }

        store.delete_all(txs.clone()).unwrap();
        store.cleanup();

        for tx in txs {
            let result = store.get(tx).unwrap();
            assert_eq!(result, None);
        }
    }

    #[test]
    fn disk_get_all_blobs() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(3);
        let txs = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs.clone())).unwrap();

        let retrieved_blobs = store.get_all(txs.clone()).unwrap();
        for (tx, blob) in retrieved_blobs {
            assert!(blobs.contains(&(tx, Arc::unwrap_or_clone(blob))));
        }

        store.delete_all(txs).unwrap();
        store.cleanup();
    }

    #[test]
    fn disk_get_exact_blobs_success() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(3);
        let txs = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs.clone())).unwrap();

        let retrieved_blobs = store.get_exact(txs).unwrap();
        for (retrieved_blob, (_, original_blob)) in retrieved_blobs.into_iter().zip(blobs) {
            assert_eq!(Arc::unwrap_or_clone(retrieved_blob), original_blob);
        }
    }

    #[test]
    fn disk_get_exact_blobs_failure() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(2);
        let txs = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs)).unwrap();

        // Try to get a blob that was never inserted
        let missing_tx = TxHash::random();
        let result = store.get_exact(vec![txs[0], missing_tx]);
        assert!(result.is_err());
    }

    #[test]
    fn disk_data_size_hint() {
        let (store, _dir) = tmp_store();
        assert_eq!(store.data_size_hint(), Some(0));

        let blobs = rng_blobs(2);
        store.insert_all(wrapped_blobs(blobs)).unwrap();
        assert!(store.data_size_hint().unwrap() > 0);
    }

    #[test]
    fn disk_cleanup_stat() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(3);
        let txs = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs)).unwrap();

        store.delete_all(txs).unwrap();
        let stat = store.cleanup();
        assert_eq!(stat.delete_succeed, 3);
        assert_eq!(stat.delete_failed, 0);
    }

    #[test]
    fn disk_get_blobs_v3_returns_partial_results() {
        let (store, _dir) = tmp_store();

        let (sidecar, versioned_hash, expected) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();

        assert_ne!(versioned_hash, B256::ZERO);

        let request = vec![versioned_hash, B256::ZERO];
        let v2 = store.get_by_versioned_hashes_v2(&request).unwrap();
        assert!(v2.is_none(), "v2 must return null if any requested blob is missing");

        let v3 = store.get_by_versioned_hashes_v3(&request).unwrap();
        assert_eq!(v3, vec![Some(expected), None]);
    }

    #[test]
    fn disk_has_blobs_returns_ordered_availability() {
        let (store, _dir) = tmp_store();

        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();

        let request = vec![B256::ZERO, versioned_hash, versioned_hash];
        assert_eq!(store.has_versioned_hashes(&request).unwrap(), vec![false, true, true]);
    }

    #[test]
    fn disk_get_blobs_v4_returns_requested_cells() {
        let (store, _dir) = tmp_store();

        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();

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
    fn disk_get_blobs_v3_can_fallback_to_disk() {
        let (store, _dir) = tmp_store();

        let (sidecar, versioned_hash, expected) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();
        store.clear_cache();

        let v3 = store.get_by_versioned_hashes_v3(&[versioned_hash]).unwrap();
        assert_eq!(v3, vec![Some(expected)]);
    }

    #[test]
    fn disk_has_blobs_can_fallback_to_disk() {
        let (store, _dir) = tmp_store();

        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();
        store.clear_cache();

        assert_eq!(store.has_versioned_hashes(&[versioned_hash]).unwrap(), vec![true]);
    }

    #[test]
    fn disk_has_blobs_ignores_stale_index_entries() {
        let (store, _dir) = tmp_store();

        let tx_hash = TxHash::random();
        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(tx_hash, sidecar.into()).unwrap();
        store.clear_cache();

        store.delete(tx_hash).unwrap();
        store.cleanup();

        assert_eq!(store.has_versioned_hashes(&[versioned_hash]).unwrap(), vec![false]);
    }

    #[test]
    fn disk_get_blobs_v4_can_fallback_to_disk() {
        let (store, _dir) = tmp_store();

        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();
        store.clear_cache();

        let v4 = store
            .get_by_versioned_hashes_v4(&[versioned_hash], BlobCellMask::from_bits(1u128))
            .unwrap();
        let cells_and_proofs = v4[0].as_ref().unwrap();
        assert_eq!(cells_and_proofs.blob_cells.len(), 1);
        assert_eq!(cells_and_proofs.proofs, vec![Some(Bytes48::default())]);
    }

    #[test]
    fn disk_get_cells_can_fallback_to_disk() {
        let (store, _dir) = tmp_store();

        let tx_hash = TxHash::random();
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

        store.clear_cache();

        assert_eq!(store.get_cells(tx_hash, indices_bitarray).unwrap(), Some(expected));
    }

    #[test]
    fn disk_double_cleanup_no_failure() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(5);
        let all_hashes: Vec<_> = blobs.iter().map(|(tx, _)| *tx).collect();
        store.insert_all(wrapped_blobs(blobs)).unwrap();
        store.clear_cache();

        // Schedule blobs for deletion
        store.delete_all(all_hashes.clone()).unwrap();

        // First cleanup: files exist, all should succeed
        let stat1 = store.cleanup();
        assert_eq!(stat1.delete_succeed, 5);
        assert_eq!(stat1.delete_failed, 0);

        // Manually re-enqueue the same hashes to simulate a concurrent cleanup race
        store.inner.txs_to_delete.write().extend(all_hashes);

        // Second cleanup: files already deleted, should still report success (NotFound)
        let stat2 = store.cleanup();
        assert_eq!(stat2.delete_succeed, 5);
        assert_eq!(stat2.delete_failed, 0);
    }
}

/// Charges the worst-case reconstructed representation up front, including heap allocations.
#[derive(Debug)]
struct SidecarCacheLimit {
    max_entries: u32,
    bytes: usize,
}
impl SidecarCacheLimit {
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    fn cost(sidecar: &PooledBlobSidecar) -> usize {
        let blobs = sidecar.versioned_hashes().count();
        sidecar.size() +
            sidecar.sidecar().size() +
            blobs * (alloy_eips::eip4844::BYTES_PER_BLOB + 129 * 48)
    }
}
impl Limiter<TxHash, Arc<PooledBlobSidecar>> for SidecarCacheLimit {
    type KeyToInsert<'a> = TxHash;
    type LinkType = u32;
    fn is_over_the_limit(&self, length: usize) -> bool {
        length > self.max_entries as usize || self.bytes > Self::MAX_BYTES
    }
    fn on_insert(
        &mut self,
        _: usize,
        key: TxHash,
        value: Arc<PooledBlobSidecar>,
    ) -> Option<(TxHash, Arc<PooledBlobSidecar>)> {
        let bytes = Self::cost(&value);
        if self.max_entries == 0 || bytes > Self::MAX_BYTES {
            return None
        }
        self.bytes += bytes;
        Some((key, value))
    }
    fn on_replace(
        &mut self,
        _: usize,
        _: &mut TxHash,
        _: TxHash,
        old: &mut Arc<PooledBlobSidecar>,
        new: &mut Arc<PooledBlobSidecar>,
    ) -> bool {
        let bytes = Self::cost(new);
        if bytes > Self::MAX_BYTES {
            return false
        }
        self.bytes = self.bytes - Self::cost(old) + bytes;
        true
    }
    fn on_removed(&mut self, _: &mut TxHash, value: &mut Arc<PooledBlobSidecar>) {
        self.bytes -= Self::cost(value);
    }
    fn on_cleared(&mut self) {
        self.bytes = 0;
    }
    fn on_grow(&mut self, _: usize) -> bool {
        true
    }
}
