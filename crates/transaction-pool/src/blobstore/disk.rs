//! A simple diskstore for blobs

use crate::blobstore::{
    cells::{merge_cells, missing_hashes},
    BlobStore, BlobStoreCleanupStat, BlobStoreError, BlobStoreSize, BlobTxCellSidecar,
    PooledBlobSidecar, RebuiltSidecars,
};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
    eip7840::BlobParams,
    merge::EPOCH_SLOTS,
};
use alloy_primitives::{
    map::{B256Map, B256Set, FbBuildHasher},
    TxHash, B256,
};
use alloy_rlp::{Decodable, Encodable};
use parking_lot::{Mutex, RwLock};
use reth_lru::LruMap;
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, trace};

/// How many [`BlobTransactionSidecarVariant`] to cache in memory.
pub const DEFAULT_MAX_CACHED_BLOBS: u32 = 100;

/// A cache size heuristic based on the highest blob params
///
/// This uses the max blobs per tx and max blobs per block over 16 epochs: `21 * 6 * 512 = 64512`
/// This should be ~4MB
const VERSIONED_HASH_TO_TX_HASH_CACHE_SIZE: u64 =
    BlobParams::bpo2().max_blobs_per_tx * BlobParams::bpo2().max_blob_count * EPOCH_SLOTS * 16;

// Prefixes cell sidecar files. Full sidecar files start with a wrapper version or a list header,
// never a string header.
const CELL_SIDECAR_TAG: u8 = alloy_rlp::EMPTY_STRING_CODE;

/// A blob store that stores blob data on disk.
///
/// The type uses deferred deletion, meaning that blobs are not immediately deleted from disk, but
/// it's expected that the maintenance task will call [`BlobStore::cleanup`] to remove the deleted
/// blobs from disk.
#[derive(Clone, Debug)]
pub struct DiskFileBlobStore {
    inner: Arc<DiskFileBlobStoreInner>,
}

impl DiskFileBlobStore {
    /// Opens and initializes a new disk file blob store according to the given options.
    pub fn open(
        blob_dir: impl Into<PathBuf>,
        opts: DiskFileBlobStoreConfig,
    ) -> Result<Self, DiskFileBlobStoreError> {
        let blob_dir = blob_dir.into();
        let DiskFileBlobStoreConfig { max_cached_entries, .. } = opts;
        let inner = DiskFileBlobStoreInner::new(blob_dir, max_cached_entries);

        // initialize the blob store
        inner.delete_all()?;
        inner.create_blob_dir()?;

        Ok(Self { inner: Arc::new(inner) })
    }

    #[cfg(test)]
    fn is_cached(&self, tx: &B256) -> bool {
        self.inner.blob_cache.lock().get(tx).is_some()
    }

    #[cfg(test)]
    fn clear_cache(&self) {
        self.inner.blob_cache.lock().clear()
    }

    /// Look up EIP-7594 blobs by their versioned hashes.
    ///
    /// This returns a result vector with the **same length and order** as the input
    /// `versioned_hashes`. Each element is `Some(BlobAndProofV2)` if the blob is available, or
    /// `None` if it is missing or an older sidecar version.
    ///
    /// The lookup first scans the in-memory cache and, if not all blobs are found, falls back to
    /// reading candidate sidecars from disk using the `versioned_hash -> tx_hash` index.
    fn get_by_versioned_hashes_eip7594(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError> {
        // we must return the blobs in order but we don't necessarily find them in the requested
        // order
        let mut result = vec![None; versioned_hashes.len()];
        let mut missing_count = result.len();
        // first scan all cached full sidecars
        for (_tx_hash, blob_sidecar) in self.inner.blob_cache.lock().iter() {
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

            // return early if all blobs are found.
            if missing_count == 0 {
                // since versioned_hashes may have duplicates, we double check here
                if result.iter().all(|blob| blob.is_some()) {
                    return Ok(result);
                }
            }
        }

        // not all versioned hashes were found, try to look up a matching tx
        let mut missing_tx_hashes = Vec::new();
        let mut seen_missing_tx_hashes = B256Set::default();

        {
            let mut versioned_to_txhashes = self.inner.versioned_hashes_to_txhash.lock();
            for (idx, _) in
                result.iter().enumerate().filter(|(_, blob_and_proof)| blob_and_proof.is_none())
            {
                // this is safe because the result vec has the same len
                let versioned_hash = versioned_hashes[idx];
                if let Some(tx_hash) = versioned_to_txhashes.get(&versioned_hash).copied() &&
                    seen_missing_tx_hashes.insert(tx_hash)
                {
                    missing_tx_hashes.push(tx_hash);
                }
            }
        }

        // if we have missing blobs, try to read them from disk and try again
        if !missing_tx_hashes.is_empty() {
            let blobs_from_disk = self.inner.read_many_decoded(missing_tx_hashes);
            for (_, stored) in blobs_from_disk {
                if let StoredSidecar::Full(blob_sidecar) = stored &&
                    let Some(blob_sidecar) = blob_sidecar.as_eip7594()
                {
                    for (hash_idx, match_result) in
                        blob_sidecar.match_versioned_hashes(versioned_hashes)
                    {
                        if result[hash_idx].is_none() {
                            result[hash_idx] = Some(match_result);
                        }
                    }
                }
            }
        }

        let mut missing = missing_hashes(versioned_hashes, &result);
        let cell_txs = self.inner.cell_txs_matching(&missing, true);
        for (tx, stored) in self.inner.read_many_decoded(cell_txs) {
            // Sidecars sharing a blob are rebuilt only while it is missing.
            if let StoredSidecar::Cells(cells) = stored &&
                cells.versioned_hashes().any(|hash| missing.contains(&hash)) &&
                let Some(blob_sidecar) = self.inner.rebuilt.get_or_rebuild(tx, &cells) &&
                let Some(blob_sidecar) = blob_sidecar.as_eip7594()
            {
                for (hash_idx, match_result) in
                    blob_sidecar.match_versioned_hashes(versioned_hashes)
                {
                    missing.remove(&versioned_hashes[hash_idx]);
                    result[hash_idx].get_or_insert(match_result);
                }
            }
        }

        Ok(result)
    }

    /// Look up EIP-7594 blob cells by their versioned hashes.
    fn get_by_versioned_hashes_cells_eip7594(
        &self,
        versioned_hashes: &[B256],
        cell_mask: BlobCellMask,
    ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError> {
        let mut result = vec![None; versioned_hashes.len()];
        let mut missing_count = result.len();

        let cached_blob_sidecars = self
            .inner
            .blob_cache
            .lock()
            .iter()
            .map(|(_, blob_sidecar)| Arc::clone(blob_sidecar))
            .collect::<Vec<_>>();
        for blob_sidecar in cached_blob_sidecars {
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

        let mut missing_tx_hashes = Vec::new();
        let mut seen_missing_tx_hashes = B256Set::default();
        {
            let mut versioned_to_txhashes = self.inner.versioned_hashes_to_txhash.lock();
            for (idx, _) in
                result.iter().enumerate().filter(|(_, cells_and_proofs)| cells_and_proofs.is_none())
            {
                let versioned_hash = versioned_hashes[idx];
                if let Some(tx_hash) = versioned_to_txhashes.get(&versioned_hash).copied() &&
                    seen_missing_tx_hashes.insert(tx_hash)
                {
                    missing_tx_hashes.push(tx_hash);
                }
            }
        }

        if !missing_tx_hashes.is_empty() {
            let blobs_from_disk = self.inner.read_many_decoded(missing_tx_hashes);
            for (_, stored) in blobs_from_disk {
                if let StoredSidecar::Full(blob_sidecar) = stored &&
                    let Some(blob_sidecar) = blob_sidecar.as_eip7594()
                {
                    for (hash_idx, match_result) in blob_sidecar
                        .match_versioned_hashes_cells(versioned_hashes, cell_mask)
                        .map_err(|err| BlobStoreError::Other(Box::new(err)))?
                    {
                        if result[hash_idx].is_none() {
                            result[hash_idx] = Some(match_result);
                        }
                    }
                }
            }
        }

        let missing = missing_hashes(versioned_hashes, &result);
        let cell_txs = self.inner.cell_txs_matching(&missing, false);
        for (_, stored) in self.inner.read_many_decoded(cell_txs) {
            if let StoredSidecar::Cells(cells) = stored {
                for (hash_idx, match_result) in
                    cells.match_versioned_hashes_cells(versioned_hashes, cell_mask)
                {
                    merge_cells(&mut result[hash_idx], match_result);
                }
            }
        }

        Ok(result)
    }
}

impl BlobStore for DiskFileBlobStore {
    fn insert(&self, tx: B256, data: PooledBlobSidecar) -> Result<(), BlobStoreError> {
        self.inner.insert_one(tx, data.into_sidecar())
    }

    fn insert_all(&self, txs: Vec<(B256, PooledBlobSidecar)>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let txs = txs.into_iter().map(|(tx, data)| (tx, data.into_sidecar())).collect();
        self.inner.insert_many(txs)
    }

    fn insert_cells(&self, tx: B256, data: BlobTxCellSidecar) -> Result<(), BlobStoreError> {
        self.inner.insert_cells(tx, data)
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        if self.inner.contains(tx)? {
            self.inner.txs_to_delete.write().insert(tx);
        }
        Ok(())
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        let txs = self.inner.retain_existing(txs)?;
        self.inner.txs_to_delete.write().extend(txs);
        Ok(())
    }

    fn cleanup(&self) -> BlobStoreCleanupStat {
        let txs_to_delete = std::mem::take(&mut *self.inner.txs_to_delete.write());
        let mut stat = BlobStoreCleanupStat::default();
        let mut subsize = 0;
        let mut removed = 0;
        debug!(target:"txpool::blob", num_blobs=%txs_to_delete.len(), "Removing blobs from disk");
        {
            let mut cell_sidecars = self.inner.cell_sidecars.write();
            for tx in &txs_to_delete {
                cell_sidecars.remove(tx);
                self.inner.rebuilt.remove(tx);
            }
        }
        for tx in txs_to_delete {
            let path = self.inner.blob_disk_file(tx);
            let filesize = fs::metadata(&path).map_or(0, |meta| meta.len());
            match fs::remove_file(&path) {
                Ok(_) => {
                    stat.delete_succeed += 1;
                    subsize += filesize;
                    removed += 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Already deleted, and uncounted, by an earlier or concurrent cleanup
                    stat.delete_succeed += 1;
                }
                Err(e) => {
                    stat.delete_failed += 1;
                    let err = DiskFileBlobStoreError::DeleteFile(tx, path, e);
                    debug!(target:"txpool::blob", %err);
                }
            };
        }
        self.inner.size_tracker.sub_size(subsize as usize);
        self.inner.size_tracker.sub_len(removed);
        stat
    }

    fn get(&self, tx: B256) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        self.inner.get_one(tx)
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        self.inner.contains(tx)
    }

    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(Vec::new())
        }
        self.inner.get_all(txs)
    }

    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(Vec::new())
        }
        self.inner.get_exact(txs)
    }

    fn get_by_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        // the response must always be the same len as the request, misses must be None
        let mut result = vec![None; versioned_hashes.len()];

        // first scan all cached full sidecars
        for (_tx_hash, blob_sidecar) in self.inner.blob_cache.lock().iter() {
            if let Some(blob_sidecar) = blob_sidecar.as_eip4844() {
                for (hash_idx, match_result) in
                    blob_sidecar.match_versioned_hashes(versioned_hashes)
                {
                    result[hash_idx] = Some(match_result);
                }
            }

            // return early if all blobs are found.
            if result.iter().all(|blob| blob.is_some()) {
                return Ok(result);
            }
        }

        // not all versioned hashes were be found, try to look up a matching tx

        let mut missing_tx_hashes = Vec::new();
        let mut seen_missing_tx_hashes = B256Set::default();

        {
            let mut versioned_to_txhashes = self.inner.versioned_hashes_to_txhash.lock();
            for (idx, _) in
                result.iter().enumerate().filter(|(_, blob_and_proof)| blob_and_proof.is_none())
            {
                // this is safe because the result vec has the same len
                let versioned_hash = versioned_hashes[idx];
                if let Some(tx_hash) = versioned_to_txhashes.get(&versioned_hash).copied() &&
                    seen_missing_tx_hashes.insert(tx_hash)
                {
                    missing_tx_hashes.push(tx_hash);
                }
            }
        }

        // if we have missing blobs, try to read them from disk and try again
        if !missing_tx_hashes.is_empty() {
            let blobs_from_disk = self.inner.read_many_decoded(missing_tx_hashes);
            for (_, stored) in blobs_from_disk {
                if let StoredSidecar::Full(blob_sidecar) = stored &&
                    let Some(blob_sidecar) = blob_sidecar.as_eip4844()
                {
                    for (hash_idx, match_result) in
                        blob_sidecar.match_versioned_hashes(versioned_hashes)
                    {
                        if result[hash_idx].is_none() {
                            result[hash_idx] = Some(match_result);
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    fn get_by_versioned_hashes_v2(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError> {
        let result = self.get_by_versioned_hashes_eip7594(versioned_hashes)?;

        // only return the blobs if we found all requested versioned hashes
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
        self.get_by_versioned_hashes_eip7594(versioned_hashes)
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
        for (_tx_hash, blob_sidecar) in self.inner.blob_cache.lock().iter() {
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

        let mut missing_tx_hashes = Vec::new();
        {
            let mut versioned_to_txhashes = self.inner.versioned_hashes_to_txhash.lock();
            for (idx, requested_hash) in versioned_hashes.iter().enumerate() {
                if !result[idx] &&
                    let Some(tx_hash) = versioned_to_txhashes.get(requested_hash).copied()
                {
                    missing_tx_hashes.push((idx, tx_hash));
                }
            }
        }

        // Held for both checks, so a promotion from cells to full in between is not missed.
        let cell_sidecars = self.inner.cell_sidecars.read();
        for (idx, tx_hash) in missing_tx_hashes {
            // The index can point to a transaction now stored as cells, which only count below.
            if !cell_sidecars.contains_key(&tx_hash) && self.inner.contains(tx_hash)? {
                result[idx] = true;
            }
        }

        // Only blobs that can be rebuilt from their cells count as available.
        for (available, requested_hash) in result.iter_mut().zip(versioned_hashes) {
            *available |= cell_sidecars
                .values()
                .any(|cells| cells.recoverable && cells.versioned_hashes.contains(requested_hash));
        }

        Ok(result)
    }

    fn get_cells(
        &self,
        tx: B256,
        cell_mask: BlobCellMask,
    ) -> Result<Option<Vec<Cell>>, BlobStoreError> {
        if self.inner.cell_sidecars.read().contains_key(&tx) {
            return Ok(match self.inner.read_one(tx)? {
                Some(StoredSidecar::Cells(cells)) => cells.get_cells(cell_mask),
                _ => None,
            })
        }

        let Some(sidecar) = self.get(tx)? else {
            return Ok(None);
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

struct DiskFileBlobStoreInner {
    blob_dir: PathBuf,
    blob_cache: Mutex<LruMap<TxHash, Arc<BlobTransactionSidecarVariant>, FbBuildHasher<32>>>,
    size_tracker: BlobStoreSize,
    file_lock: RwLock<()>,
    txs_to_delete: RwLock<B256Set>,
    /// Tracks of known versioned hashes and a transaction they exist in
    ///
    /// Note: It is possible that one blob can appear in multiple transactions but this only tracks
    /// the most recent one.
    versioned_hashes_to_txhash: Mutex<LruMap<B256, B256, FbBuildHasher<32>>>,
    // Transactions stored as cell sidecars. They are not in `versioned_hashes_to_txhash`, which
    // only points to full sidecars.
    cell_sidecars: RwLock<B256Map<CellSidecarFile>>,
    // Sidecars rebuilt from cell sidecars.
    rebuilt: RebuiltSidecars,
}

impl DiskFileBlobStoreInner {
    /// Creates a new empty disk file blob store with the given maximum length of the blob cache.
    fn new(blob_dir: PathBuf, max_length: u32) -> Self {
        Self {
            blob_dir,
            blob_cache: Mutex::new(LruMap::with_hasher(max_length, Default::default())),
            size_tracker: Default::default(),
            file_lock: Default::default(),
            txs_to_delete: Default::default(),
            versioned_hashes_to_txhash: Mutex::new(LruMap::with_hasher(
                VERSIONED_HASH_TO_TX_HASH_CACHE_SIZE as u32,
                Default::default(),
            )),
            cell_sidecars: Default::default(),
            rebuilt: Default::default(),
        }
    }

    /// Creates the directory where blobs will be stored on disk.
    fn create_blob_dir(&self) -> Result<(), DiskFileBlobStoreError> {
        debug!(target:"txpool::blob", blob_dir = ?self.blob_dir, "Creating blob store");
        fs::create_dir_all(&self.blob_dir)
            .map_err(|e| DiskFileBlobStoreError::Open(self.blob_dir.clone(), e))
    }

    /// Deletes the entire blob store.
    fn delete_all(&self) -> Result<(), DiskFileBlobStoreError> {
        match fs::remove_dir_all(&self.blob_dir) {
            Ok(_) => {
                debug!(target:"txpool::blob", blob_dir = ?self.blob_dir, "Removed blob store directory");
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(DiskFileBlobStoreError::Open(self.blob_dir.clone(), err)),
        }
        Ok(())
    }

    /// Ensures blob is in the blob cache and written to the disk.
    fn insert_one(
        &self,
        tx: B256,
        data: BlobTransactionSidecarVariant,
    ) -> Result<(), BlobStoreError> {
        let mut buf = Vec::with_capacity(data.rlp_encoded_fields_length());
        data.rlp_encode_fields(&mut buf);

        {
            // cache the versioned hashes to tx hash
            let mut map = self.versioned_hashes_to_txhash.lock();
            data.versioned_hashes().for_each(|hash| {
                map.insert(hash, tx);
            });
        }

        self.blob_cache.lock().insert(tx, Arc::new(data));

        let size = self.write_one_encoded(tx, &buf)?;

        self.size_tracker.add_size(size);
        self.size_tracker.inc_len(1);
        Ok(())
    }

    /// Ensures blobs are in the blob cache and written to the disk.
    fn insert_many(
        &self,
        txs: Vec<(B256, BlobTransactionSidecarVariant)>,
    ) -> Result<(), BlobStoreError> {
        let raw = txs
            .iter()
            .map(|(tx, data)| {
                let mut buf = Vec::with_capacity(data.rlp_encoded_fields_length());
                data.rlp_encode_fields(&mut buf);
                (*tx, self.blob_disk_file(*tx), buf)
            })
            .collect::<Vec<_>>();

        {
            // cache versioned hashes to tx hash
            let mut map = self.versioned_hashes_to_txhash.lock();
            for (tx, data) in &txs {
                data.versioned_hashes().for_each(|hash| {
                    map.insert(hash, *tx);
                });
            }
        }

        {
            // cache blobs
            let mut cache = self.blob_cache.lock();
            for (tx, data) in txs {
                cache.insert(tx, Arc::new(data));
            }
        }

        let mut add = 0;
        let mut num = 0;
        {
            let _lock = self.file_lock.write();
            for (tx, path, data) in raw {
                let replaced = match self.replace_cells(tx, &path, &data) {
                    Ok(replaced) => replaced,
                    Err(err) => {
                        debug!(target:"txpool::blob", %err, ?path, "Failed to replace cell file");
                        continue
                    }
                };
                if replaced {
                    add += data.len();
                    num += 1;
                } else if path.exists() {
                    debug!(target:"txpool::blob", ?path, "Blob already exists");
                } else if let Err(err) = fs::write(&path, &data) {
                    debug!(target:"txpool::blob", %err, ?path, "Failed to write blob file");
                } else {
                    add += data.len();
                    num += 1;
                }
            }
        }
        self.size_tracker.add_size(add);
        self.size_tracker.inc_len(num);

        Ok(())
    }

    // Replaces the cell file of `tx` at `path` with its full sidecar, returning `false` if there
    // is none. The cell file is kept if this fails. Called with `file_lock` held for writing.
    fn replace_cells(&self, tx: B256, path: &Path, data: &[u8]) -> io::Result<bool> {
        if !self.cell_sidecars.read().contains_key(&tx) {
            return Ok(false)
        }
        let size = fs::metadata(path).map_or(0, |meta| meta.len());
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, data)?;
        fs::rename(&tmp, path)?;
        self.cell_sidecars.write().remove(&tx);
        self.rebuilt.remove(&tx);
        self.size_tracker.sub_size(size as usize);
        self.size_tracker.sub_len(1);
        Ok(true)
    }

    // Writes a cell sidecar to disk, unless the transaction already has a sidecar.
    fn insert_cells(&self, tx: B256, data: BlobTxCellSidecar) -> Result<(), BlobStoreError> {
        let path = self.blob_disk_file(tx);
        // The blob cache can outlive a deleted sidecar, so only the file counts.
        if path.is_file() {
            return Ok(())
        }
        let mut buf = Vec::with_capacity(1 + data.length());
        buf.push(CELL_SIDECAR_TAG);
        data.encode(&mut buf);

        let file = CellSidecarFile {
            recoverable: data.is_recoverable(),
            versioned_hashes: data.versioned_hashes().collect(),
        };
        {
            // Recorded and counted under the lock, so a full sidecar written next replaces this
            // file and its size.
            let _lock = self.file_lock.write();
            // A full sidecar may have been written since.
            if path.exists() {
                return Ok(())
            }
            fs::write(&path, &buf).map_err(|e| DiskFileBlobStoreError::WriteFile(tx, path, e))?;
            self.cell_sidecars.write().insert(tx, file);
            self.size_tracker.add_size(buf.len());
            self.size_tracker.inc_len(1);
        }
        Ok(())
    }

    // Returns the transactions stored as cells with a blob in `hashes`, only those whose blobs
    // can be rebuilt if `recoverable`.
    fn cell_txs_matching(&self, hashes: &B256Set, recoverable: bool) -> Vec<TxHash> {
        if hashes.is_empty() {
            return Vec::new()
        }
        self.cell_sidecars
            .read()
            .iter()
            .filter(|(_, cells)| {
                (cells.recoverable || !recoverable) &&
                    cells.versioned_hashes.iter().any(|hash| hashes.contains(hash))
            })
            .map(|(tx, _)| *tx)
            .collect()
    }

    /// Returns true if the blob for the given transaction hash is in the blob cache or on disk.
    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        if self.blob_cache.lock().get(&tx).is_some() {
            return Ok(true)
        }
        // we only check if the file exists and assume it's valid
        Ok(self.blob_disk_file(tx).is_file())
    }

    /// Returns all the blob transactions which are in the cache or on the disk.
    fn retain_existing(&self, txs: Vec<B256>) -> Result<Vec<B256>, BlobStoreError> {
        let (in_cache, not_in_cache): (Vec<B256>, Vec<B256>) = {
            let mut cache = self.blob_cache.lock();
            txs.into_iter().partition(|tx| cache.get(tx).is_some())
        };

        let mut existing = in_cache;
        for tx in not_in_cache {
            if self.blob_disk_file(tx).is_file() {
                existing.push(tx);
            }
        }

        Ok(existing)
    }

    /// Retrieves the blob for the given transaction hash from the blob cache or disk.
    fn get_one(
        &self,
        tx: B256,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        if let Some(blob) = self.blob_cache.lock().get(&tx) {
            return Ok(Some(blob.clone()))
        }

        Ok(self.read_one(tx)?.and_then(|stored| self.full_sidecar(tx, stored)))
    }

    // Returns a full sidecar read from disk, caching it, or rebuilds one from cells.
    fn full_sidecar(
        &self,
        tx: B256,
        stored: StoredSidecar,
    ) -> Option<Arc<BlobTransactionSidecarVariant>> {
        match stored {
            StoredSidecar::Full(blob) => {
                let blob_arc = Arc::new(blob);
                self.blob_cache.lock().insert(tx, blob_arc.clone());
                Some(blob_arc)
            }
            StoredSidecar::Cells(cells) => self.rebuilt.get_or_rebuild(tx, &cells),
        }
    }

    /// Returns the path to the blob file for the given transaction hash.
    #[inline]
    fn blob_disk_file(&self, tx: B256) -> PathBuf {
        self.blob_dir.join(format!("{tx:x}"))
    }

    /// Retrieves the blob data for the given transaction hash.
    #[inline]
    fn read_one(&self, tx: B256) -> Result<Option<StoredSidecar>, BlobStoreError> {
        let path = self.blob_disk_file(tx);
        let data = {
            let _lock = self.file_lock.read();
            match fs::read(&path) {
                Ok(data) => data,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => {
                    return Err(BlobStoreError::Other(Box::new(DiskFileBlobStoreError::ReadFile(
                        tx, path, e,
                    ))))
                }
            }
        };
        StoredSidecar::decode(&data).map(Some).map_err(BlobStoreError::DecodeError)
    }

    /// Returns decoded blobs read from disk.
    ///
    /// Only returns sidecars that were found and successfully decoded.
    fn read_many_decoded(&self, txs: Vec<TxHash>) -> Vec<(TxHash, StoredSidecar)> {
        self.read_many_raw(txs)
            .into_iter()
            .filter_map(|(tx, data)| StoredSidecar::decode(&data).map(|sidecar| (tx, sidecar)).ok())
            .collect()
    }

    /// Retrieves the raw blob data for the given transaction hashes.
    ///
    /// Only returns the blobs that were found in file.
    #[inline]
    fn read_many_raw(&self, txs: Vec<TxHash>) -> Vec<(TxHash, Vec<u8>)> {
        let mut res = Vec::with_capacity(txs.len());
        let _lock = self.file_lock.read();
        for tx in txs {
            let path = self.blob_disk_file(tx);
            match fs::read(&path) {
                Ok(data) => {
                    res.push((tx, data));
                }
                Err(err) => {
                    debug!(target:"txpool::blob", %err, ?tx, "Failed to read blob file");
                }
            };
        }
        res
    }

    /// Writes the blob data for the given transaction hash to the disk, replacing its cell file.
    #[inline]
    fn write_one_encoded(&self, tx: B256, data: &[u8]) -> Result<usize, DiskFileBlobStoreError> {
        trace!(target:"txpool::blob", "[{:?}] writing blob file", tx);
        let mut add = 0;
        let path = self.blob_disk_file(tx);
        {
            let _lock = self.file_lock.write();
            if self
                .replace_cells(tx, &path, data)
                .map_err(|e| DiskFileBlobStoreError::WriteFile(tx, path.clone(), e))?
            {
                add = data.len();
            } else if !path.exists() {
                fs::write(&path, data)
                    .map_err(|e| DiskFileBlobStoreError::WriteFile(tx, path, e))?;
                add = data.len();
            }
        }
        Ok(add)
    }

    /// Retrieves blobs for the given transaction hashes from the blob cache or disk.
    ///
    /// This will not return an error if there are missing blobs. Therefore, the result may be a
    /// subset of the request or an empty vector if none of the blobs were found.
    #[inline]
    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        let mut res = Vec::with_capacity(txs.len());
        let mut cache_miss = Vec::new();
        {
            let mut cache = self.blob_cache.lock();
            for tx in txs {
                if let Some(blob) = cache.get(&tx) {
                    res.push((tx, blob.clone()));
                } else {
                    cache_miss.push(tx)
                }
            }
        }
        if cache_miss.is_empty() {
            return Ok(res)
        }
        let from_disk = self.read_many_decoded(cache_miss);
        if from_disk.is_empty() {
            return Ok(res)
        }
        let mut from_cells = Vec::new();
        let from_disk = from_disk
            .into_iter()
            .filter_map(|(tx, stored)| match stored {
                StoredSidecar::Full(data) => {
                    let data = Arc::new(data);
                    res.push((tx, data.clone()));
                    Some((tx, data))
                }
                StoredSidecar::Cells(cells) => {
                    from_cells.push((tx, cells));
                    None
                }
            })
            .collect::<Vec<_>>();

        {
            let mut cache = self.blob_cache.lock();
            for (tx, data) in from_disk {
                cache.insert(tx, data);
            }
        }
        for (tx, cells) in from_cells {
            if let Some(data) = self.rebuilt.get_or_rebuild(tx, &cells) {
                res.push((tx, data));
            }
        }

        Ok(res)
    }

    /// Retrieves blobs for the given transaction hashes from the blob cache or disk.
    ///
    /// Returns an error if there are any missing blobs.
    #[inline]
    fn get_exact(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        txs.into_iter()
            .map(|tx| self.get_one(tx)?.ok_or(BlobStoreError::MissingSidecar(tx)))
            .collect()
    }
}

impl fmt::Debug for DiskFileBlobStoreInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiskFileBlobStoreInner")
            .field("blob_dir", &self.blob_dir)
            .field("cached_blobs", &self.blob_cache.try_lock().map(|lock| lock.len()))
            .field("txs_to_delete", &self.txs_to_delete.try_read())
            .finish()
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

// What the disk store keeps in memory about a cell sidecar file.
#[derive(Debug)]
struct CellSidecarFile {
    // Whether the blobs can be rebuilt from the kept cells.
    recoverable: bool,
    versioned_hashes: Vec<B256>,
}

// A sidecar decoded from its blob file.
enum StoredSidecar {
    Full(BlobTransactionSidecarVariant),
    Cells(BlobTxCellSidecar),
}

impl StoredSidecar {
    fn decode(data: &[u8]) -> alloy_rlp::Result<Self> {
        match data.split_first() {
            Some((&CELL_SIDECAR_TAG, mut cells)) => {
                BlobTxCellSidecar::decode(&mut cells).map(Self::Cells)
            }
            _ => BlobTransactionSidecarVariant::rlp_decode_fields(&mut &data[..]).map(Self::Full),
        }
    }
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
    use crate::blobstore::cells::tests::{fixture, subset};
    use std::sync::atomic::Ordering;

    fn tmp_store() -> (DiskFileBlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskFileBlobStore::open(dir.path(), Default::default()).unwrap();
        (store, dir)
    }

    // A store that caches no full sidecars, so reads go to disk.
    fn tmp_uncached_store() -> (DiskFileBlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = DiskFileBlobStoreConfig::default().with_max_cached_entries(0);
        let store = DiskFileBlobStore::open(dir.path(), config).unwrap();
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

    // One cell of the blob in `eip7594_single_blob_sidecar`, with its placeholder proofs.
    fn single_cell_sidecar() -> BlobTxCellSidecar {
        BlobTxCellSidecar::new(
            vec![Bytes48::default()],
            vec![Bytes48::default(); CELLS_PER_EXT_BLOB],
            vec![Cell::default()],
            BlobCellMask::from_bits(1),
        )
        .unwrap()
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
    fn disk_queued_deletions_count_until_cleanup() {
        let (store, _dir) = tmp_store();
        assert_eq!(store.blobs_len(), 0);

        let blobs = rng_blobs(2);
        let hashes = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(wrapped_blobs(blobs)).unwrap();

        // queued deletions keep the blobs counted, so the store isn't reported empty before
        // cleanup ran
        store.delete_all(vec![hashes[0]]).unwrap();
        assert_eq!(store.blobs_len(), 2);

        store.cleanup();
        assert_eq!(store.blobs_len(), 1);

        // the removed blob is still cached, so it can be queued again
        store.delete(hashes[0]).unwrap();
        store.cleanup();
        assert_eq!(store.blobs_len(), 1);
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
        assert_eq!(
            result,
            Some(Arc::new(BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar {
                blobs: vec![],
                commitments: vec![],
                proofs: vec![]
            })))
        );
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
            assert_eq!(
                result,
                Some(Arc::new(BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar {
                    blobs: vec![],
                    commitments: vec![],
                    proofs: vec![]
                })))
            );
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

        store.clear_cache();

        assert_eq!(store.get_cells(tx_hash, cell_mask).unwrap(), Some(expected));
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

    #[test]
    fn disk_cell_files_are_tagged_and_cleaned_up() {
        let (store, _dir) = tmp_store();
        let tx = TxHash::random();
        let cells = single_cell_sidecar();
        store.insert_cells(tx, cells.clone()).unwrap();
        let file = fs::read(store.inner.blob_disk_file(tx)).unwrap();
        assert_eq!(file, [&[CELL_SIDECAR_TAG][..], &alloy_rlp::encode(&cells)].concat());

        store.delete(tx).unwrap();
        store.cleanup();
        assert!(store.inner.cell_sidecars.read().is_empty());
    }

    #[test]
    fn disk_get_blobs_v3_does_not_cache_disk_reads() {
        let (store, _dir) = tmp_store();
        let (sidecar, versioned_hash, expected) = eip7594_single_blob_sidecar();
        let tx = TxHash::random();
        store.insert(tx, sidecar.into()).unwrap();
        store.clear_cache();

        assert_eq!(
            store.get_by_versioned_hashes_v3(&[versioned_hash]).unwrap(),
            vec![Some(expected)]
        );
        assert!(!store.is_cached(&tx));
    }

    #[test]
    fn disk_cells_keep_the_index_on_a_full_blob() {
        let (store, _dir) = tmp_uncached_store();
        let (sidecar, versioned_hash, expected) = eip7594_single_blob_sidecar();
        store.insert(TxHash::random(), sidecar.into()).unwrap();
        // Another transaction keeps one cell of the same blob.
        store.insert_cells(TxHash::random(), single_cell_sidecar()).unwrap();

        assert_eq!(store.has_versioned_hashes(&[versioned_hash]).unwrap(), vec![true]);
        assert_eq!(
            store.get_by_versioned_hashes_v3(&[versioned_hash]).unwrap(),
            vec![Some(expected)]
        );
    }

    #[test]
    fn disk_failed_cell_write_is_not_recorded() {
        let (store, dir) = tmp_store();
        fs::remove_dir_all(dir.path()).unwrap();
        let tx = TxHash::random();

        let Err(BlobStoreError::Other(err)) = store.insert_cells(tx, single_cell_sidecar()) else {
            panic!("expected a write error")
        };
        assert!(matches!(
            err.downcast_ref::<DiskFileBlobStoreError>(),
            Some(DiskFileBlobStoreError::WriteFile(failed, _, _)) if *failed == tx
        ));
        assert!(store.inner.cell_sidecars.read().is_empty());
        assert_eq!(store.blobs_len(), 0);
    }

    #[test]
    fn disk_failed_replacement_keeps_the_cell_file() {
        let (store, _dir) = tmp_uncached_store();
        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        let tx = TxHash::random();
        let cells = single_cell_sidecar();
        store.insert_cells(tx, cells.clone()).unwrap();
        let cell_mask = BlobCellMask::from_bits(1);

        // A directory at the staging path fails the write, even when run as root.
        fs::create_dir(store.inner.blob_disk_file(tx).with_extension("tmp")).unwrap();
        let result = store.insert(tx, sidecar.into());

        let Err(BlobStoreError::Other(err)) = result else { panic!("expected a write error") };
        assert!(matches!(
            err.downcast_ref::<DiskFileBlobStoreError>(),
            Some(DiskFileBlobStoreError::WriteFile(failed, _, _)) if *failed == tx
        ));
        assert_eq!(store.get_cells(tx, cell_mask).unwrap(), cells.get_cells(cell_mask));
        // The full sidecar was indexed, but the cells cannot rebuild its blob.
        assert_eq!(store.has_versioned_hashes(&[versioned_hash]).unwrap(), vec![false]);
    }

    #[test]
    fn disk_cells_are_stored_over_a_deleted_cached_sidecar() {
        let (store, _dir) = tmp_store();
        let (sidecar, _, _) = eip7594_single_blob_sidecar();
        let tx = TxHash::random();
        store.insert(tx, sidecar.into()).unwrap();
        store.delete(tx).unwrap();
        store.cleanup();
        assert!(store.is_cached(&tx));

        let cells = single_cell_sidecar();
        store.insert_cells(tx, cells.clone()).unwrap();
        // The cells outlive the eviction of the deleted sidecar.
        store.clear_cache();
        let cell_mask = BlobCellMask::from_bits(1);
        assert_eq!(store.get_cells(tx, cell_mask).unwrap(), cells.get_cells(cell_mask));
    }

    #[test]
    fn disk_has_blobs_ignores_index_entries_of_cell_files() {
        let (store, _dir) = tmp_uncached_store();
        let (sidecar, versioned_hash, _) = eip7594_single_blob_sidecar();
        let tx = TxHash::random();
        store.insert(tx, sidecar.into()).unwrap();
        store.delete(tx).unwrap();
        store.cleanup();
        // The index still points to `tx`, which comes back with one cell.
        store.insert_cells(tx, single_cell_sidecar()).unwrap();

        assert_eq!(store.get_by_versioned_hashes_v3(&[versioned_hash]).unwrap(), vec![None]);
        assert_eq!(store.has_versioned_hashes(&[versioned_hash]).unwrap(), vec![false]);
    }

    #[test]
    fn disk_get_blobs_v3_rebuilds_a_shared_blob_once() {
        let (store, _dir) = tmp_store();
        let (_, all) = fixture();
        let hash = all.versioned_hashes().next().unwrap();
        let half = subset(BlobCellMask::from_bits(u128::MAX / 3));
        store.insert_cells(B256::random(), half.clone()).unwrap();
        store.insert_cells(B256::random(), half).unwrap();

        assert!(store.get_by_versioned_hashes_v3(&[hash]).unwrap()[0].is_some());
        assert_eq!(store.inner.rebuilt.cache.lock().sidecars.len(), 1);
    }
}
