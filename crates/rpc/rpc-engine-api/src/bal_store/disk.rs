//! Disk-backed BAL store.

use crate::bal_store::{BalStore, BalStoreError};
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::PathBuf,
    sync::Arc,
};
use tracing::debug;

/// Default maximum retained BAL entries on disk.
///
/// Roughly aligns with the weak subjectivity period target discussed in EIP-7928.
pub const DEFAULT_MAX_BAL_STORE_ENTRIES: u32 = 113_000;

/// Configuration for [`DiskFileBalStore`].
#[derive(Debug, Clone, Copy)]
pub struct DiskFileBalStoreConfig {
    /// Maximum number of BAL entries to retain.
    pub max_entries: u32,
}

impl Default for DiskFileBalStoreConfig {
    fn default() -> Self {
        Self { max_entries: DEFAULT_MAX_BAL_STORE_ENTRIES }
    }
}

impl DiskFileBalStoreConfig {
    /// Sets the maximum retained entries.
    pub const fn with_max_entries(mut self, max_entries: u32) -> Self {
        self.max_entries = max_entries;
        self
    }
}

/// A disk-backed BAL store with in-memory indexes.
///
/// BAL payloads are stored as raw bytes under `<dir>/entries/<blockhash_hex>`.
/// Block-number index entries are stored under `<dir>/index/<block_number>` with
/// 32-byte block hash content.
#[derive(Clone, Debug)]
pub struct DiskFileBalStore {
    inner: Arc<DiskFileBalStoreInner>,
}

#[derive(Debug, Default)]
struct IndexState {
    block_to_hash: BTreeMap<BlockNumber, BlockHash>,
    hash_to_block: HashMap<BlockHash, BlockNumber>,
}

#[derive(Debug)]
struct DiskFileBalStoreInner {
    root_dir: PathBuf,
    entries_dir: PathBuf,
    index_dir: PathBuf,
    max_entries: u32,
    state: Mutex<IndexState>,
}

impl DiskFileBalStore {
    /// Opens (or creates) a disk BAL store and rebuilds in-memory indexes.
    pub fn open(
        bal_dir: impl Into<PathBuf>,
        opts: DiskFileBalStoreConfig,
    ) -> Result<Self, BalStoreError> {
        let root_dir = bal_dir.into();
        let entries_dir = root_dir.join("entries");
        let index_dir = root_dir.join("index");

        fs::create_dir_all(&entries_dir)?;
        fs::create_dir_all(&index_dir)?;

        let inner = DiskFileBalStoreInner {
            root_dir,
            entries_dir,
            index_dir,
            max_entries: opts.max_entries,
            state: Mutex::new(IndexState::default()),
        };
        inner.rebuild_indexes()?;

        Ok(Self { inner: Arc::new(inner) })
    }
}

impl BalStore for DiskFileBalStore {
    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> Result<(), BalStoreError> {
        self.inner.insert(block_hash, block_number, bal)
    }

    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> Result<Vec<Option<Bytes>>, BalStoreError> {
        self.inner.get_by_hashes(block_hashes)
    }

    fn get_by_range(&self, start: BlockNumber, count: u64) -> Result<Vec<Bytes>, BalStoreError> {
        self.inner.get_by_range(start, count)
    }
}

impl DiskFileBalStoreInner {
    fn rebuild_indexes(&self) -> Result<(), BalStoreError> {
        let mut indexed = Vec::new();

        let dir = match fs::read_dir(&self.index_dir) {
            Ok(dir) => dir,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        };

        for entry in dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    debug!(target: "rpc::engine", %err, "Failed to read BAL index dir entry");
                    continue;
                }
            };

            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|f| f.to_str()) else {
                debug!(target: "rpc::engine", ?path, "Skipping BAL index entry with non-utf8 name");
                continue;
            };

            let Ok(block_number) = file_name.parse::<BlockNumber>() else {
                debug!(target: "rpc::engine", ?path, "Skipping BAL index entry with invalid block number name");
                continue;
            };

            let hash_bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    debug!(target: "rpc::engine", ?path, %err, "Failed reading BAL index file");
                    continue;
                }
            };

            if hash_bytes.len() != 32 {
                debug!(
                    target: "rpc::engine",
                    ?path,
                    len = hash_bytes.len(),
                    "Skipping BAL index file with invalid hash length"
                );
                continue;
            }

            let block_hash = BlockHash::from_slice(&hash_bytes);
            if !self.entry_file(block_hash).is_file() {
                debug!(
                    target: "rpc::engine",
                    block_number,
                    ?block_hash,
                    "Skipping BAL index entry pointing to missing BAL payload file"
                );
                continue;
            }

            indexed.push((block_number, block_hash));
        }

        indexed.sort_unstable_by_key(|(number, _)| *number);

        let mut state = self.state.lock();
        state.block_to_hash.clear();
        state.hash_to_block.clear();

        for (block_number, block_hash) in indexed {
            if let Some(old_number) = state.hash_to_block.insert(block_hash, block_number) {
                state.block_to_hash.remove(&old_number);
                let _ = self.remove_if_exists(self.index_file(old_number));
            }
            state.block_to_hash.insert(block_number, block_hash);
        }

        self.evict_over_capacity(&mut state)?;
        Ok(())
    }

    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> Result<(), BalStoreError> {
        let mut state = self.state.lock();

        // If the hash was previously indexed at another number, move the index.
        if let Some(old_number) = state.hash_to_block.get(&block_hash).copied() &&
            old_number != block_number
        {
            state.block_to_hash.remove(&old_number);
            self.remove_if_exists(self.index_file(old_number))?;
        }

        // Reorg replacement: remove old hash payload at this number.
        if let Some(old_hash) = state.block_to_hash.get(&block_number).copied() &&
            old_hash != block_hash
        {
            state.hash_to_block.remove(&old_hash);
            self.remove_if_exists(self.entry_file(old_hash))?;
        }

        fs::write(self.entry_file(block_hash), bal.as_ref())?;
        fs::write(self.index_file(block_number), block_hash.as_slice())?;

        state.block_to_hash.insert(block_number, block_hash);
        state.hash_to_block.insert(block_hash, block_number);

        self.evict_over_capacity(&mut state)
    }

    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> Result<Vec<Option<Bytes>>, BalStoreError> {
        block_hashes
            .iter()
            .map(|hash| {
                let path = self.entry_file(*hash);
                match fs::read(path) {
                    Ok(bytes) => Ok(Some(Bytes::from(bytes))),
                    Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(err) => Err(err.into()),
                }
            })
            .collect()
    }

    fn get_by_range(&self, start: BlockNumber, count: u64) -> Result<Vec<Bytes>, BalStoreError> {
        let hashes: Vec<BlockHash> = {
            let state = self.state.lock();
            let mut hashes = Vec::new();
            for number in start..start.saturating_add(count) {
                let Some(hash) = state.block_to_hash.get(&number).copied() else {
                    break;
                };
                hashes.push(hash);
            }
            hashes
        };

        let mut result = Vec::with_capacity(hashes.len());
        for hash in hashes {
            match fs::read(self.entry_file(hash)) {
                Ok(bytes) => result.push(Bytes::from(bytes)),
                Err(err) if err.kind() == io::ErrorKind::NotFound => break,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(result)
    }

    fn evict_over_capacity(&self, state: &mut IndexState) -> Result<(), BalStoreError> {
        while state.block_to_hash.len() as u32 > self.max_entries {
            let Some((&oldest_number, &oldest_hash)) = state.block_to_hash.first_key_value() else {
                break;
            };

            state.block_to_hash.remove(&oldest_number);
            state.hash_to_block.remove(&oldest_hash);

            self.remove_if_exists(self.entry_file(oldest_hash))?;
            self.remove_if_exists(self.index_file(oldest_number))?;
        }
        Ok(())
    }

    fn remove_if_exists(&self, path: PathBuf) -> Result<(), BalStoreError> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    #[inline]
    fn entry_file(&self, block_hash: BlockHash) -> PathBuf {
        self.entries_dir.join(format!("{block_hash:x}"))
    }

    #[inline]
    fn index_file(&self, block_number: BlockNumber) -> PathBuf {
        self.index_dir.join(block_number.to_string())
    }
}

impl std::fmt::Display for DiskFileBalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DiskFileBalStore({})", self.inner.root_dir.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    fn tmp_store(max_entries: u32) -> (DiskFileBalStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskFileBalStore::open(
            dir.path(),
            DiskFileBalStoreConfig::default().with_max_entries(max_entries),
        )
        .unwrap();
        (store, dir)
    }

    #[test]
    fn insert_and_get_hashes() {
        let (store, _dir) = tmp_store(10);
        let h1 = B256::random();
        let h2 = B256::random();

        store.insert(h1, 1, Bytes::from_static(b"a")).unwrap();
        store.insert(h2, 2, Bytes::from_static(b"b")).unwrap();

        let got = store.get_by_hashes(&[h1, h2, B256::random()]).unwrap();
        assert_eq!(got, vec![Some(Bytes::from_static(b"a")), Some(Bytes::from_static(b"b")), None]);
    }

    #[test]
    fn range_stops_on_gap() {
        let (store, _dir) = tmp_store(10);
        store.insert(B256::random(), 1, Bytes::from_static(b"a")).unwrap();
        store.insert(B256::random(), 2, Bytes::from_static(b"b")).unwrap();
        store.insert(B256::random(), 4, Bytes::from_static(b"d")).unwrap();

        let got = store.get_by_range(1, 10).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn reorg_replaces_entry() {
        let (store, _dir) = tmp_store(10);
        let old_hash = B256::random();
        let new_hash = B256::random();

        store.insert(old_hash, 42, Bytes::from_static(b"old")).unwrap();
        store.insert(new_hash, 42, Bytes::from_static(b"new")).unwrap();

        let got_old = store.get_by_hashes(&[old_hash]).unwrap();
        let got_new = store.get_by_hashes(&[new_hash]).unwrap();
        assert_eq!(got_old, vec![None]);
        assert_eq!(got_new, vec![Some(Bytes::from_static(b"new"))]);
    }

    #[test]
    fn evicts_oldest() {
        let (store, _dir) = tmp_store(2);
        store.insert(B256::random(), 10, Bytes::from_static(b"a")).unwrap();
        store.insert(B256::random(), 20, Bytes::from_static(b"b")).unwrap();
        store.insert(B256::random(), 30, Bytes::from_static(b"c")).unwrap();

        let got = store.get_by_range(10, 1).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn recovers_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let h1 = B256::random();
        let h2 = B256::random();
        {
            let store = DiskFileBalStore::open(dir.path(), Default::default()).unwrap();
            store.insert(h1, 100, Bytes::from_static(b"a")).unwrap();
            store.insert(h2, 101, Bytes::from_static(b"b")).unwrap();
        }

        let reopened = DiskFileBalStore::open(dir.path(), Default::default()).unwrap();
        let got = reopened.get_by_hashes(&[h1, h2]).unwrap();
        assert_eq!(got, vec![Some(Bytes::from_static(b"a")), Some(Bytes::from_static(b"b"))]);
    }
}
