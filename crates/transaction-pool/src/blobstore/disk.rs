use parking_lot::RwLock;
use reth_primitives::{B256, BlobTransactionSidecar, TxHash};
use schnellru::{ByLength, LruMap};
use std::{fmt, fs, io, path::PathBuf, sync::Arc};
use tracing::debug;
use crate::{BlobStore, BlobStoreError};
use crate::blobstore::BlobStoreSize;

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
        let DiskFileBlobStoreConfig { max_cached_entries, open } = opts;
        if blob_dir.exists() {
            match open {
                OpenDiskFileBlobStore::Index => {
                    // TODO read all blobs
                }
                OpenDiskFileBlobStore::Clear => {
                    // delete the blob store
                    debug!(target:"txpool", ?blob_dir, "Clearing blob store");
                    fs::remove_dir_all(&blob_dir)
                        .map_err(|e| DiskFileBlobStoreError::FailedToOpen(blob_dir.clone(), e))?;
                    fs::create_dir(&blob_dir)
                        .map_err(|e| DiskFileBlobStoreError::FailedToOpen(blob_dir.clone(), e))?;
                }
            }
        }
        Ok(Self { inner: Arc::new(DiskFileBlobStoreInner::new(blob_dir, max_cached_entries)) })
    }
}

impl BlobStore for DiskFileBlobStore {
    fn insert(&self, tx: B256, data: BlobTransactionSidecar) -> Result<(), BlobStoreError> {
        todo!()
    }

    fn insert_all(&self, txs: Vec<(B256, BlobTransactionSidecar)>) -> Result<(), BlobStoreError> {
        todo!()
    }

    fn delete(&self, tx: B256) -> Result<(), BlobStoreError> {
        todo!()
    }

    fn delete_all(&self, txs: Vec<B256>) -> Result<(), BlobStoreError> {
        todo!()
    }

    fn get(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        todo!()
    }

    fn get_all(&self, txs: Vec<B256>) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError> {
        todo!()
    }

    fn get_exact(&self, txs: Vec<B256>) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        todo!()
    }

    fn data_size_hint(&self) -> Option<usize> {
        Some(
            self.inner.size_tracker.data_size()
        )
    }

    fn blobs_len(&self) -> usize {
        self.inner.size_tracker.blobs_len()
    }
}

struct DiskFileBlobStoreInner {
    blob_dir: PathBuf,
    blob_cache: RwLock<LruMap<TxHash, BlobTransactionSidecar, ByLength>>,
    size_tracker: BlobStoreSize,
}

impl DiskFileBlobStoreInner {
    /// Creates a new empty disk file blob store with the given maximum length of the blob cache.
    fn new(blob_dir: PathBuf, max_length: u32) -> Self {
        Self { blob_dir, blob_cache: RwLock::new(LruMap::new(ByLength::new(max_length))), size_tracker: Default::default() }
    }

    /// Retrieves the blob for the given transaction hash from the blob cache or disk.
    fn get_one(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        if let Some(blob) = self.blob_cache.read().get(&tx) {
            return Ok(Some(blob.clone()))
        }
        let blob = self.read_one(tx)?;
        if let Some(blob) = &blob {
            self.blob_cache.write().insert(tx, blob.clone());
        }
        Ok(blob)
    }

    /// Returns the path to the blob file for the given transaction hash.
    fn blob_disk_file(&self, tx: B256) -> PathBuf {
        self.blob_dir.join(format!("{:x}", tx))
    }

    /// Retries the blob data for the given transaction hash.
    #[inline]
    fn read_one(&self, tx: B256) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        let path = self.blob_disk_file(tx);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(BlobStoreError::Other(Box::new(
                DiskFileBlobStoreError::FailedToReadBlobFile(tx, path, e),
            ))),
        };
        BlobTransactionSidecar::decode(&data).map(Some).map_err(BlobStoreError::DecodeError)
        todo!()
    }


    fn get_all(&self, txs: Vec<B256>) -> Result<Vec<(B256, BlobTransactionSidecar)>, BlobStoreError> {
        todo!()
    }

}

impl fmt::Debug for DiskFileBlobStoreInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiskFileBlobStoreInner")
            .field("blob_dir", &self.blob_dir)
            .field("cached_blobs", &self.blob_cache.read().len())
            .finish()
    }
}

/// Errors that can occur when interacting with a disk file blob store.
#[derive(Debug, thiserror::Error)]
pub enum DiskFileBlobStoreError {
    /// Thrown during [DiskFileBlobStore::open] if the blob store directory cannot be opened.
    #[error("Failed to blobstore at {0:?}: {1:?}")]
    FailedToOpen(PathBuf, io::Error),
    #[error("[0] Failed read blob file at {1:?}: {2:?}")]
    FailedToReadBlobFile(TxHash, PathBuf, io::Error),
}

/// Configuration for a disk file blob store.
#[derive(Debug, Clone, Default)]
pub struct DiskFileBlobStoreConfig {
    /// The maximum number of blobs to keep in the in memory blob cache.
    pub max_cached_entries: u32,
    /// How to open the blob store.
    pub open: OpenDiskFileBlobStore,
}

/// How to open a disk file blob store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenDiskFileBlobStore {
    /// Clear everything in the blob store.
    #[default]
    Clear,
    /// Keep the existing blob store and index
    Index,
}
