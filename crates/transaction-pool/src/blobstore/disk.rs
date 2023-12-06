//! A simple diskstore for blobs

use crate::{blobstore::BlobStoreSize, BlobStore, BlobStoreError};
use alloy_rlp::{Decodable, Encodable};
use parking_lot::{Mutex, RwLock};
use reth_primitives::{BlobTransactionSidecar, TxHash, B256};
use schnellru::{ByLength, LruMap};
use std::{fmt, fs, io, path::PathBuf, sync::Arc};
use tracing::{debug, trace};

/// How many [BlobTransactionSidecar] to cache in memory.
pub const DEFAULT_MAX_CACHED_BLOBS: u32 = 100;

/// A blob store that stores blob data on disk.
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
}

impl BlobStore for DiskFileBlobStore {
    fn insert(&self, tx: B256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        self.inner.insert_one(tx, data)
    }

    fn insert_all(&self, txs: Vec<(B256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }
        self.inner.insert_many(txs)
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        self.inner.delete_one(tx)?;
        Ok(())
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        if txs.is_empty() {
            return Ok(())
        }

        self.inner.delete_many(txs)?;
        Ok(())
    }

    fn get(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        self.inner.get_one(tx)
    }

    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        self.inner.contains(tx)
    }

    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(Vec::new())
        }
        self.inner.get_all(txs)
    }

    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        if txs.is_empty() {
            return Ok(Vec::new())
        }
        self.inner.get_exact(txs)
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
    blob_cache: Mutex<LruMap<TxHash, BlobTransactionSidecar, ByLength>>,
    size_tracker: BlobStoreSize,
    file_lock: RwLock<()>,
}

impl DiskFileBlobStoreInner {
    /// Creates a new empty disk file blob store with the given maximum length of the blob cache.
    fn new(blob_dir: PathBuf, max_length: u32) -> Self {
        Self {
            blob_dir,
            blob_cache: Mutex::new(LruMap::new(ByLength::new(max_length))),
            size_tracker: Default::default(),
            file_lock: Default::default(),
        }
    }

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

    fn insert_one(&self, tx: B256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        let mut buf = Vec::with_capacity(data.fields_len());
        data.encode(&mut buf);
        self.blob_cache.lock().insert(tx, data);
        let size = self.write_one_encoded(tx, &buf)?;

        self.size_tracker.add_size(size);
        self.size_tracker.inc_len(1);
        Ok(())
    }

    fn insert_many(&self, txs: Vec<(B256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        let raw = txs
            .iter()
            .map(|(tx, data)| {
                let mut buf = Vec::with_capacity(data.fields_len());
                data.encode(&mut buf);
                (self.blob_disk_file(*tx), buf)
            })
            .collect::<Vec<_>>();

        {
            let mut cache = self.blob_cache.lock();
            for (tx, data) in txs {
                cache.insert(tx, data);
            }
        }
        let mut add = 0;
        let mut num = 0;
        {
            let _lock = self.file_lock.write();
            for (path, data) in raw {
                if let Err(err) = fs::write(&path, &data) {
                    debug!( target:"txpool::blob", ?err, ?path, "Failed to write blob file");
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

    /// Returns true if the blob for the given transaction hash is in the blob cache or on disk.
    fn contains(&self, tx: B256) -> Result<bool, BlobStoreError> {
        if self.blob_cache.lock().get(&tx).is_some() {
            return Ok(true)
        }
        // we only check if the file exists and assume it's valid
        Ok(self.blob_disk_file(tx).is_file())
    }

    /// Retrieves the blob for the given transaction hash from the blob cache or disk.
    fn get_one(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        if let Some(blob) = self.blob_cache.lock().get(&tx) {
            return Ok(Some(blob.clone()))
        }
        let blob = self.read_one(tx)?;
        if let Some(blob) = &blob {
            self.blob_cache.lock().insert(tx, blob.clone());
        }
        Ok(blob)
    }

    /// Returns the path to the blob file for the given transaction hash.
    #[inline]
    fn blob_disk_file(&self, tx: B256) -> PathBuf {
        self.blob_dir.join(format!("{:x}", tx))
    }

    /// Retries the blob data for the given transaction hash.
    #[inline]
    fn read_one(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
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
        BlobTransactionSidecar::decode(&mut data.as_slice())
            .map(Some)
            .map_err(BlobStoreError::DecodeError)
    }

    /// Returns decoded blobs read from disk.
    fn read_many_decoded(&self, txs: Vec<TxHash>) -> Vec<(TxHash, BlobTransactionSidecar)> {
        self.read_many_raw(txs)
            .into_iter()
            .filter_map(|(tx, data)| {
                BlobTransactionSidecar::decode(&mut data.as_slice())
                    .map(|sidecar| (tx, sidecar))
                    .ok()
            })
            .collect()
    }

    /// Retries the raw blob data for the given transaction hashes.
    ///
    /// Only returns the blobs that were found on file.
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
                    debug!( target:"txpool::blob", ?err, ?tx, "Failed to read blob file");
                }
            };
        }
        res
    }

    /// Retries the blob data for the given transaction hash.
    #[inline]
    fn write_one_encoded(&self, tx: B256, data: &[u8]) -> Result<usize, DiskFileBlobStoreError> {
        trace!( target:"txpool::blob", "[{:?}] writing blob file", tx);
        let path = self.blob_disk_file(tx);
        let _lock = self.file_lock.write();

        fs::write(&path, data).map_err(|e| DiskFileBlobStoreError::WriteFile(tx, path, e))?;
        Ok(data.len())
    }

    /// Retries the blob data for the given transaction hash.
    #[inline]
    fn delete_one(&self, tx: B256) -> Result<(), DiskFileBlobStoreError> {
        trace!( target:"txpool::blob", "[{:?}] deleting blob file", tx);
        let path = self.blob_disk_file(tx);

        let _lock = self.file_lock.write();
        fs::remove_file(&path).map_err(|e| DiskFileBlobStoreError::WriteFile(tx, path, e))?;

        Ok(())
    }

    /// Retries the blob data for the given transaction hash.
    #[inline]
    fn delete_many(
        &self,
        txs: impl IntoIterator<Item = TxHash>,
    ) -> Result<(), DiskFileBlobStoreError> {
        let _lock = self.file_lock.write();
        for tx in txs.into_iter() {
            trace!( target:"txpool::blob", "[{:?}] deleting blob file", tx);
            let path = self.blob_disk_file(tx);

            let _ = fs::remove_file(&path).map_err(|e| {
                let err = DiskFileBlobStoreError::WriteFile(tx, path, e);
                debug!( target:"txpool::blob", ?err);
            });
        }

        Ok(())
    }

    #[inline]
    fn get_all(
        &self,
        txs: Vec<B256>,
    ) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError> {
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
        let mut cache = self.blob_cache.lock();
        for (tx, data) in from_disk {
            cache.insert(tx, data.clone());
            res.push((tx, data));
        }

        Ok(res)
    }

    #[inline]
    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        let mut res = Vec::with_capacity(txs.len());
        for tx in txs {
            let blob = self.get_one(tx)?.ok_or_else(|| BlobStoreError::MissingSidecar(tx))?;
            res.push(blob)
        }

        Ok(res)
    }
}

impl fmt::Debug for DiskFileBlobStoreInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiskFileBlobStoreInner")
            .field("blob_dir", &self.blob_dir)
            .field("cached_blobs", &self.blob_cache.try_lock().map(|lock| lock.len()))
            .finish()
    }
}

/// Errors that can occur when interacting with a disk file blob store.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum DiskFileBlobStoreError {
    /// Thrown during [DiskFileBlobStore::open] if the blob store directory cannot be opened.
    #[error("failed to open blobstore at {0}: {1}")]
    Open(PathBuf, io::Error),
    #[error("[{0}] failed to read blob file at {1}: {2}")]
    ReadFile(TxHash, PathBuf, io::Error),
    #[error("[{0}] failed to write blob file at {1}: {2}")]
    WriteFile(TxHash, PathBuf, io::Error),
    #[error("[{0}] failed to delete blob file at {1}: {2}")]
    DeleteFile(TxHash, PathBuf, io::Error),
}

impl From<DiskFileBlobStoreError> for BlobStoreError {
    fn from(value: DiskFileBlobStoreError) -> Self {
        BlobStoreError::Other(Box::new(value))
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
    use super::*;
    use proptest::{
        prelude::*,
        strategy::{Strategy, ValueTree},
        test_runner::TestRunner,
    };

    fn tmp_store() -> (DiskFileBlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskFileBlobStore::open(dir.path(), Default::default()).unwrap();
        (store, dir)
    }

    fn rng_blobs(num: usize) -> Vec<(TxHash, BlobTransactionSidecar)> {
        let mut runner = TestRunner::new(Default::default());
        prop::collection::vec(any::<(TxHash, BlobTransactionSidecar)>(), num)
            .new_tree(&mut runner)
            .unwrap()
            .current()
    }

    #[test]
    fn disk_insert_all_get_all() {
        let (store, _dir) = tmp_store();

        let blobs = rng_blobs(10);
        let all_hashes = blobs.iter().map(|(tx, _)| *tx).collect::<Vec<_>>();
        store.insert_all(blobs.clone()).unwrap();
        // all cached
        for (tx, blob) in &blobs {
            assert!(store.is_cached(tx));
            assert_eq!(store.get(*tx).unwrap().unwrap(), *blob);
        }
        let all = store.get_all(all_hashes.clone()).unwrap();
        for (tx, blob) in all {
            assert!(blobs.contains(&(tx, blob)), "missing blob {:?}", tx);
        }

        assert!(store.contains(all_hashes[0]).unwrap());
        store.delete_all(all_hashes.clone()).unwrap();
        store.clear_cache();

        assert!(store.get(blobs[0].0).unwrap().is_none());

        let all = store.get_all(all_hashes.clone()).unwrap();
        assert!(all.is_empty());

        assert!(!store.contains(all_hashes[0]).unwrap());
        assert!(store.get_exact(all_hashes).is_err());
    }
}
