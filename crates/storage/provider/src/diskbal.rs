use alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS;
use alloy_eips::NumHash;
use alloy_primitives::{BlockHash, BlockNumber, Bytes, B256};
use parking_lot::{Mutex, RwLock};
use reth_prune_types::PruneMode;
use reth_storage_api::{
    BalNotification, BalNotificationStream, BalStore, GetBlockAccessListLimit, SealedBal,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_tokio_util::EventSender;
use schnellru::{ByLength, LruMap};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Size of the bounded best-effort BAL notification channel.
const DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE: usize = 256;

/// Default number of blocks grouped into one disk BAL chunk.
pub const DEFAULT_DISK_BAL_CHUNK_SIZE: u64 = 10_000;
/// Default number of BAL payloads cached in memory by the disk BAL store.
pub const DEFAULT_DISK_BAL_CACHE_SIZE: u32 = 500;

const DISK_BAL_STORE_VERSION_DIR: &str = "v1";
const DISK_BAL_DATA_FILE_NAME: &str = "data.bin";
const DISK_BAL_INDEX_FILE_NAME: &str = "index.bin";
const DISK_BAL_INDEX_ENTRY_LEN: usize = 84;

/// Disk-backed BAL store using sparse block-number chunks.
///
/// This store is optimized for hash lookups while still making retention cheap. The on-disk data is
/// grouped by block-number chunks, and a compact in-memory hash index is rebuilt from those chunk
/// indexes whenever the store opens.
///
/// # Storage model
///
/// `DiskBalStore::open(root, config)` owns a versioned subdirectory under `root`:
///
/// ```text
/// root/
///   v1/
///     00000000000000000000-00000000000000009999/
///       data.bin
///       index.bin
///     00000000000000010000-00000000000000019999/
///       data.bin
///       index.bin
/// ```
///
/// Each chunk directory covers `config.chunk_size` block numbers. `data.bin` is an append-only byte
/// stream of BAL payloads. `index.bin` is an append-only sequence of fixed-size entries containing:
/// block number, block hash, BAL hash, payload offset, and payload length.
///
/// The store does not require block numbers to be contiguous. Hash lookups use the in-memory
/// `BlockHash -> (chunk, offset, len)` index. Range lookups walk block numbers from the requested
/// start and stop at the first gap.
///
/// Retention is applied by removing complete chunk directories whose chunk end is outside the
/// configured retention window. The current chunk is kept until the whole chunk ages out.
///
/// The store also keeps a bounded in-memory LRU cache of recent BAL payloads. Inserts populate this
/// cache immediately, and reads that miss the cache populate it after loading from disk.
///
/// Inserts are buffered in memory. Call [`Self::flush`] to append pending BALs to `data.bin` and
/// `index.bin`, and to remove chunk directories queued by retention.
#[derive(Debug, Clone)]
pub struct DiskBalStore {
    config: DiskBalStoreConfig,
    inner: Arc<RwLock<DiskBalStoreInner>>,
    cache: Arc<Mutex<LruMap<BlockHash, Bytes, ByLength>>>,
    notifications: EventSender<BalNotification>,
}

impl DiskBalStore {
    /// Opens a disk BAL store rooted at the given path.
    ///
    /// Existing chunk indexes are scanned to rebuild the compact in-memory lookup tables. Missing
    /// or malformed chunk directories are ignored, partial trailing index entries are ignored, and
    /// index entries that point outside their `data.bin` are skipped.
    ///
    /// The implementation owns a `v1` subdirectory under `root`, but it is otherwise standalone and
    /// not wired into node setup.
    pub fn open(root: impl Into<PathBuf>, config: DiskBalStoreConfig) -> ProviderResult<Self> {
        if config.chunk_size == 0 {
            return Err(ProviderError::other(io::Error::new(
                io::ErrorKind::InvalidInput,
                "disk BAL chunk size must be non-zero",
            )))
        }

        let root = root.into().join(DISK_BAL_STORE_VERSION_DIR);
        reth_fs_util::create_dir_all(&root).map_err(ProviderError::other)?;

        let mut inner = DiskBalStoreInner::load(root, config.chunk_size)?;
        inner.prune(config.retention, true)?;

        Ok(Self {
            config,
            inner: Arc::new(RwLock::new(inner)),
            cache: Arc::new(Mutex::new(LruMap::new(ByLength::new(config.max_cached_entries)))),
            notifications: EventSender::new(DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE),
        })
    }

    /// Reads a BAL by hash, using the in-memory LRU cache before falling back to disk.
    fn read_by_hash_cached(&self, block_hash: BlockHash) -> ProviderResult<Option<Bytes>> {
        if let Some(bal) = self.cache.lock().get(&block_hash).cloned() {
            return Ok(Some(bal))
        }

        let bal = {
            let inner = self.inner.read();
            inner.read_by_hash(block_hash)?
        };

        if let Some(bal) = &bal {
            self.cache.lock().insert(block_hash, bal.clone());
        }

        Ok(bal)
    }

    /// Flushes all pending BAL inserts to disk.
    ///
    /// This appends visible pending BALs to their chunk files, converts them into durable
    /// locations, applies retention, and deletes any chunk directories that were queued for
    /// deferred removal by prior inserts.
    pub fn flush(&self) -> ProviderResult<()> {
        let mut inner = self.inner.write();
        inner.flush_pending()?;
        inner.prune(self.config.retention, true)?;
        inner.delete_queued_chunks()?;
        Ok(())
    }

    #[cfg(test)]
    fn is_cached(&self, block_hash: &BlockHash) -> bool {
        self.cache.lock().peek(block_hash).is_some()
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache.lock().len()
    }

    #[cfg(test)]
    fn clear_cache(&self) {
        self.cache.lock().clear()
    }
}

impl BalStore for DiskBalStore {
    /// Adds a BAL to the pending overlay, updates the hash lookup index, and notifies subscribers.
    fn insert(&self, num_hash: NumHash, bal: SealedBal) -> ProviderResult<()> {
        let bal_bytes = bal.clone_inner();
        let bal_hash = bal.hash();

        {
            let mut inner = self.inner.write();
            inner.insert_pending(num_hash, bal_hash, bal_bytes.clone());
            inner.prune(self.config.retention, false)?;
        }
        self.cache.lock().insert(num_hash.hash, bal_bytes);
        self.notifications.notify(BalNotification::new(num_hash, bal));
        Ok(())
    }

    fn flush(&self) -> ProviderResult<()> {
        Self::flush(self)
    }

    /// Returns BAL payloads for the requested block hashes, preserving request order.
    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let mut result = Vec::with_capacity(block_hashes.len());

        for hash in block_hashes {
            result.push(self.read_by_hash_cached(*hash)?);
        }

        Ok(result)
    }

    /// Appends hash lookup results to `out` until the soft response limit is exceeded.
    ///
    /// Missing BALs are represented by the empty-list RLP payload to match the in-memory store.
    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let mut size = 0;

        for hash in block_hashes {
            let bal =
                self.read_by_hash_cached(*hash)?.unwrap_or_else(|| Bytes::from_static(&[0xc0]));
            size += bal.len();
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }

        Ok(())
    }

    /// Returns a contiguous range of BALs by block number.
    ///
    /// The disk store allows gaps, so the returned range stops at the first missing block number or
    /// missing payload.
    fn get_by_range(&self, start: BlockNumber, count: u64) -> ProviderResult<Vec<Bytes>> {
        let mut result = Vec::new();

        for offset in 0..count {
            let Some(block_number) = start.checked_add(offset) else { break };
            let block_hash = {
                let inner = self.inner.read();
                inner.hashes_by_number.get(&block_number).and_then(|hashes| hashes.first()).copied()
            };
            let Some(block_hash) = block_hash else { break };
            let Some(bal) = self.read_by_hash_cached(block_hash)? else { break };
            result.push(bal);
        }

        Ok(result)
    }

    fn bal_stream(&self) -> BalNotificationStream {
        self.notifications.new_listener()
    }
}

/// Configuration for the disk-backed BAL store.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DiskBalStoreConfig {
    /// Number of block numbers grouped into a single chunk directory.
    pub chunk_size: u64,
    /// Retention policy applied by deleting complete chunks.
    pub retention: Option<PruneMode>,
    /// Maximum number of BAL payloads kept in the in-memory LRU cache.
    pub max_cached_entries: u32,
}

impl DiskBalStoreConfig {
    /// Returns a config with no disk BAL retention limit.
    ///
    /// The default chunk size is still used. Existing chunks are loaded on open and new chunks are
    /// never removed by this store.
    pub const fn unbounded() -> Self {
        Self {
            chunk_size: DEFAULT_DISK_BAL_CHUNK_SIZE,
            retention: None,
            max_cached_entries: DEFAULT_DISK_BAL_CACHE_SIZE,
        }
    }

    /// Returns a config with the given disk BAL retention policy.
    ///
    /// Retention deletes only complete chunk directories. If the retention boundary lands inside a
    /// chunk, that chunk remains available until its end block is prunable.
    pub const fn with_retention(retention: PruneMode) -> Self {
        Self {
            chunk_size: DEFAULT_DISK_BAL_CHUNK_SIZE,
            retention: Some(retention),
            max_cached_entries: DEFAULT_DISK_BAL_CACHE_SIZE,
        }
    }

    /// Sets the number of block numbers grouped into a chunk directory.
    ///
    /// Smaller chunks reduce stale data retained past the pruning boundary, while larger chunks
    /// reduce directory count and index files. A zero chunk size is rejected by
    /// [`DiskBalStore::open`].
    pub const fn with_chunk_size(mut self, chunk_size: u64) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Sets the maximum number of BAL payloads kept in the in-memory LRU cache.
    ///
    /// A value of zero disables payload caching while keeping the disk-backed hash index active.
    pub const fn with_max_cached_entries(mut self, max_cached_entries: u32) -> Self {
        self.max_cached_entries = max_cached_entries;
        self
    }
}

impl Default for DiskBalStoreConfig {
    fn default() -> Self {
        Self::with_retention(PruneMode::Distance(BAL_RETENTION_PERIOD_SLOTS))
    }
}

#[derive(Debug)]
struct DiskBalStoreInner {
    root: PathBuf,
    chunk_size: u64,
    entries: HashMap<BlockHash, DiskBalEntry>,
    hashes_by_number: BTreeMap<BlockNumber, Vec<BlockHash>>,
    hashes_by_chunk: BTreeMap<BlockNumber, Vec<BlockHash>>,
    chunks_pending_delete: BTreeSet<BlockNumber>,
    highest_block_number: Option<BlockNumber>,
}

impl DiskBalStoreInner {
    /// Rebuilds the in-memory lookup tables by scanning valid chunk directories under `root`.
    fn load(root: PathBuf, chunk_size: u64) -> ProviderResult<Self> {
        let mut inner = Self {
            root,
            chunk_size,
            entries: HashMap::new(),
            hashes_by_number: BTreeMap::new(),
            hashes_by_chunk: BTreeMap::new(),
            chunks_pending_delete: BTreeSet::new(),
            highest_block_number: None,
        };

        for entry in reth_fs_util::read_dir(&inner.root).map_err(ProviderError::other)? {
            let entry = entry.map_err(ProviderError::other)?;
            if !entry.file_type().map_err(ProviderError::other)?.is_dir() {
                continue
            }

            let Some(chunk_name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            let Some((chunk_start, chunk_end)) = parse_disk_bal_chunk_dir_name(&chunk_name) else {
                continue;
            };
            if chunk_end != disk_bal_chunk_end(chunk_start, chunk_size) {
                continue
            }

            inner.load_chunk_index(chunk_start, chunk_end)?;
        }

        Ok(inner)
    }

    /// Loads one chunk's `index.bin` and registers entries that are valid for that chunk.
    ///
    /// Entries are ignored if the block number is outside the directory's encoded range, if the
    /// referenced payload extends past `data.bin`, or if they are part of a trailing partial index
    /// entry left by an interrupted write.
    fn load_chunk_index(
        &mut self,
        chunk_start: BlockNumber,
        chunk_end: BlockNumber,
    ) -> ProviderResult<()> {
        let index_path = self.index_path(chunk_start);
        let index = match reth_fs_util::read(&index_path) {
            Ok(index) => index,
            Err(reth_fs_util::FsPathError::Read { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(())
            }
            Err(err) => return Err(ProviderError::other(err)),
        };

        let data_path = self.data_path(chunk_start);
        let data_len = match reth_fs_util::metadata(&data_path) {
            Ok(metadata) => metadata.len(),
            Err(reth_fs_util::FsPathError::Metadata { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                0
            }
            Err(err) => return Err(ProviderError::other(err)),
        };

        for entry in index.chunks_exact(DISK_BAL_INDEX_ENTRY_LEN).map(DiskBalIndexEntry::decode) {
            if !(chunk_start..=chunk_end).contains(&entry.block_number) {
                continue
            }
            if entry.end_offset().is_none_or(|end| end > data_len) {
                continue
            }

            self.insert_location(
                entry.block_hash,
                DiskBalEntry::Flushed(DiskBalLocation {
                    block_number: entry.block_number,
                    chunk_start,
                    offset: entry.offset,
                    len: entry.len,
                }),
            );
        }

        Ok(())
    }

    /// Appends a BAL payload and a matching fixed-size index entry to the owning chunk.
    ///
    /// The returned location is not added to the in-memory index by this method. Callers update the
    /// index only after both disk appends succeed.
    fn append(
        &self,
        num_hash: NumHash,
        bal_hash: B256,
        bal: &[u8],
    ) -> ProviderResult<DiskBalLocation> {
        let len = u32::try_from(bal.len()).map_err(|_| {
            ProviderError::other(io::Error::new(
                io::ErrorKind::InvalidInput,
                "BAL payload exceeds disk BAL store entry length",
            ))
        })?;
        let chunk_start = disk_bal_chunk_start(num_hash.number, self.chunk_size);
        let chunk_dir = self.chunk_dir(chunk_start);
        reth_fs_util::create_dir_all(&chunk_dir).map_err(ProviderError::other)?;

        let data_path = self.data_path(chunk_start);
        let mut data_file =
            OpenOptions::new().create(true).append(true).read(true).open(&data_path).map_err(
                |err| ProviderError::other(reth_fs_util::FsPathError::open(err, &data_path)),
            )?;
        let offset = data_file.metadata().map_err(ProviderError::other)?.len();
        data_file.write_all(bal).map_err(|err| {
            ProviderError::other(reth_fs_util::FsPathError::write(err, &data_path))
        })?;
        data_file.flush().map_err(|err| {
            ProviderError::other(reth_fs_util::FsPathError::write(err, &data_path))
        })?;

        let index_path = self.index_path(chunk_start);
        let mut index_file =
            OpenOptions::new().create(true).append(true).open(&index_path).map_err(|err| {
                ProviderError::other(reth_fs_util::FsPathError::open(err, &index_path))
            })?;
        let index_entry = DiskBalIndexEntry {
            block_number: num_hash.number,
            block_hash: num_hash.hash,
            bal_hash,
            offset,
            len,
        };
        index_file.write_all(&index_entry.encode()).map_err(|err| {
            ProviderError::other(reth_fs_util::FsPathError::write(err, &index_path))
        })?;
        index_file.flush().map_err(|err| {
            ProviderError::other(reth_fs_util::FsPathError::write(err, &index_path))
        })?;

        Ok(DiskBalLocation { block_number: num_hash.number, chunk_start, offset, len })
    }

    /// Registers a block hash in the in-memory indexes, replacing any previous entry.
    fn insert_location(&mut self, block_hash: BlockHash, entry: DiskBalEntry) {
        if let Some(previous) = self.entries.insert(block_hash, entry.clone()) {
            self.remove_hash_from_number(previous.block_number(), block_hash);
            self.remove_hash_from_chunk(previous.chunk_start(), block_hash);
        }

        let block_number = entry.block_number();
        self.hashes_by_number.entry(block_number).or_default().push(block_hash);
        self.hashes_by_chunk.entry(entry.chunk_start()).or_default().push(block_hash);
        self.highest_block_number = Some(
            self.highest_block_number.map_or(block_number, |highest| highest.max(block_number)),
        );
    }

    /// Registers a pending BAL without writing it to disk.
    fn insert_pending(&mut self, num_hash: NumHash, bal_hash: B256, bal: Bytes) {
        let entry = DiskBalEntry::Pending(PendingDiskBal {
            block_number: num_hash.number,
            chunk_start: disk_bal_chunk_start(num_hash.number, self.chunk_size),
            bal_hash,
            bal,
        });
        self.insert_location(num_hash.hash, entry);
    }

    /// Reads a BAL by block hash using the in-memory location index.
    fn read_by_hash(&self, block_hash: BlockHash) -> ProviderResult<Option<Bytes>> {
        let Some(entry) = self.entries.get(&block_hash) else { return Ok(None) };

        match entry {
            DiskBalEntry::Flushed(location) => self.read_location(*location).map(Some),
            DiskBalEntry::Pending(pending) => Ok(Some(pending.bal.clone())),
        }
    }

    /// Reads the byte range described by a previously validated disk location.
    fn read_location(&self, location: DiskBalLocation) -> ProviderResult<Bytes> {
        let data_path = self.data_path(location.chunk_start);
        let mut file = OpenOptions::new().read(true).open(&data_path).map_err(|err| {
            ProviderError::other(reth_fs_util::FsPathError::open(err, &data_path))
        })?;
        file.seek(SeekFrom::Start(location.offset)).map_err(ProviderError::other)?;

        let mut data = vec![0; location.len as usize];
        file.read_exact(&mut data).map_err(|err| {
            ProviderError::other(reth_fs_util::FsPathError::read(err, &data_path))
        })?;
        Ok(Bytes::from(data))
    }

    /// Applies retention by deleting fully prunable chunk directories.
    ///
    /// Chunks are evaluated by their end block, so this never removes a chunk that may still
    /// contain a retained block.
    fn prune(
        &mut self,
        prune_mode: Option<PruneMode>,
        delete_from_disk: bool,
    ) -> ProviderResult<()> {
        let Some(prune_mode) = prune_mode else { return Ok(()) };
        let Some(tip) = self.highest_block_number else { return Ok(()) };

        let mut chunks_to_remove = Vec::new();
        for &chunk_start in self.hashes_by_chunk.keys() {
            let chunk_end = disk_bal_chunk_end(chunk_start, self.chunk_size);
            if prune_mode.should_prune(chunk_end, tip) {
                chunks_to_remove.push(chunk_start);
            } else {
                break
            }
        }

        for chunk_start in chunks_to_remove {
            if let Some(hashes) = self.hashes_by_chunk.remove(&chunk_start) {
                for hash in hashes {
                    let Some(location) = self
                        .entries
                        .get(&hash)
                        .filter(|entry| entry.chunk_start() == chunk_start)
                        .cloned()
                    else {
                        continue;
                    };
                    self.entries.remove(&hash);
                    self.remove_hash_from_number(location.block_number(), hash);
                }
            }

            if delete_from_disk {
                self.delete_chunk_dir(chunk_start)?;
            } else {
                self.chunks_pending_delete.insert(chunk_start);
            }
        }

        Ok(())
    }

    /// Appends all visible pending entries to disk and converts them into durable locations.
    fn flush_pending(&mut self) -> ProviderResult<()> {
        let pending = self
            .entries
            .iter()
            .filter_map(|(&block_hash, entry)| match entry {
                DiskBalEntry::Pending(pending) => Some((block_hash, pending.clone())),
                DiskBalEntry::Flushed(_) => None,
            })
            .collect::<Vec<_>>();

        for (block_hash, pending) in pending {
            let location = self.append(
                NumHash::new(pending.block_number, block_hash),
                pending.bal_hash,
                pending.bal.as_ref(),
            )?;
            if let Some(entry) = self.entries.get_mut(&block_hash) {
                *entry = DiskBalEntry::Flushed(location);
            }
        }

        Ok(())
    }

    /// Deletes chunk directories queued by insert-time retention.
    fn delete_queued_chunks(&mut self) -> ProviderResult<()> {
        let chunks = self.chunks_pending_delete.iter().copied().collect::<Vec<_>>();
        for chunk_start in chunks {
            self.delete_chunk_dir(chunk_start)?;
            self.chunks_pending_delete.remove(&chunk_start);
        }
        Ok(())
    }

    /// Deletes one chunk directory if it exists.
    fn delete_chunk_dir(&self, chunk_start: BlockNumber) -> ProviderResult<()> {
        let chunk_dir = self.chunk_dir(chunk_start);
        match reth_fs_util::remove_dir_all(&chunk_dir) {
            Ok(()) => Ok(()),
            Err(reth_fs_util::FsPathError::RemoveDir { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                Ok(())
            }
            Err(err) => Err(ProviderError::other(err)),
        }
    }

    /// Removes a block hash from the block-number index.
    fn remove_hash_from_number(&mut self, block_number: BlockNumber, block_hash: BlockHash) {
        let Some(hashes) = self.hashes_by_number.get_mut(&block_number) else { return };
        hashes.retain(|hash| *hash != block_hash);
        if hashes.is_empty() {
            self.hashes_by_number.remove(&block_number);
        }
    }

    /// Removes a block hash from the chunk index.
    fn remove_hash_from_chunk(&mut self, chunk_start: BlockNumber, block_hash: BlockHash) {
        let Some(hashes) = self.hashes_by_chunk.get_mut(&chunk_start) else { return };
        hashes.retain(|hash| *hash != block_hash);
        if hashes.is_empty() {
            self.hashes_by_chunk.remove(&chunk_start);
        }
    }

    /// Returns the path to a chunk directory by chunk start.
    fn chunk_dir(&self, chunk_start: BlockNumber) -> PathBuf {
        disk_bal_chunk_dir(&self.root, chunk_start, self.chunk_size)
    }

    /// Returns the path to a chunk's payload file.
    fn data_path(&self, chunk_start: BlockNumber) -> PathBuf {
        self.chunk_dir(chunk_start).join(DISK_BAL_DATA_FILE_NAME)
    }

    /// Returns the path to a chunk's fixed-size index file.
    fn index_path(&self, chunk_start: BlockNumber) -> PathBuf {
        self.chunk_dir(chunk_start).join(DISK_BAL_INDEX_FILE_NAME)
    }
}

/// Location of one BAL payload inside a chunk's `data.bin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskBalLocation {
    block_number: BlockNumber,
    chunk_start: BlockNumber,
    offset: u64,
    len: u32,
}

/// BAL entry that is either already flushed to disk or pending in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DiskBalEntry {
    Flushed(DiskBalLocation),
    Pending(PendingDiskBal),
}

impl DiskBalEntry {
    /// Returns the block number this entry belongs to.
    const fn block_number(&self) -> BlockNumber {
        match self {
            Self::Flushed(location) => location.block_number,
            Self::Pending(pending) => pending.block_number,
        }
    }

    /// Returns the first block of the chunk this entry belongs to.
    const fn chunk_start(&self) -> BlockNumber {
        match self {
            Self::Flushed(location) => location.chunk_start,
            Self::Pending(pending) => pending.chunk_start,
        }
    }
}

/// BAL payload buffered by [`DiskBalStore::insert`] until [`DiskBalStore::flush`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingDiskBal {
    block_number: BlockNumber,
    chunk_start: BlockNumber,
    bal_hash: B256,
    bal: Bytes,
}

/// Fixed-size `index.bin` entry describing one appended BAL payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskBalIndexEntry {
    block_number: BlockNumber,
    block_hash: BlockHash,
    bal_hash: B256,
    offset: u64,
    len: u32,
}

impl DiskBalIndexEntry {
    /// Encodes an index entry as little-endian fixed-width bytes.
    fn encode(self) -> [u8; DISK_BAL_INDEX_ENTRY_LEN] {
        let mut out = [0; DISK_BAL_INDEX_ENTRY_LEN];
        out[0..8].copy_from_slice(&self.block_number.to_le_bytes());
        out[8..40].copy_from_slice(self.block_hash.as_slice());
        out[40..72].copy_from_slice(self.bal_hash.as_slice());
        out[72..80].copy_from_slice(&self.offset.to_le_bytes());
        out[80..84].copy_from_slice(&self.len.to_le_bytes());
        out
    }

    /// Decodes an index entry from exactly [`DISK_BAL_INDEX_ENTRY_LEN`] bytes.
    fn decode(bytes: &[u8]) -> Self {
        let mut block_number = [0; 8];
        block_number.copy_from_slice(&bytes[0..8]);
        let mut offset = [0; 8];
        offset.copy_from_slice(&bytes[72..80]);
        let mut len = [0; 4];
        len.copy_from_slice(&bytes[80..84]);

        Self {
            block_number: u64::from_le_bytes(block_number),
            block_hash: B256::from_slice(&bytes[8..40]),
            bal_hash: B256::from_slice(&bytes[40..72]),
            offset: u64::from_le_bytes(offset),
            len: u32::from_le_bytes(len),
        }
    }

    /// Returns the exclusive end offset of this entry's payload.
    const fn end_offset(&self) -> Option<u64> {
        self.offset.checked_add(self.len as u64)
    }
}

/// Returns the first block number in the chunk containing `block_number`.
const fn disk_bal_chunk_start(block_number: BlockNumber, chunk_size: u64) -> BlockNumber {
    block_number / chunk_size * chunk_size
}

/// Returns the last block number in a chunk.
const fn disk_bal_chunk_end(chunk_start: BlockNumber, chunk_size: u64) -> BlockNumber {
    chunk_start.saturating_add(chunk_size.saturating_sub(1))
}

/// Returns the path to a chunk directory under `root`.
fn disk_bal_chunk_dir(root: &Path, chunk_start: BlockNumber, chunk_size: u64) -> PathBuf {
    let chunk_end = disk_bal_chunk_end(chunk_start, chunk_size);
    root.join(format!("{chunk_start:020}-{chunk_end:020}"))
}

/// Parses a chunk directory name of the form `{start:020}-{end:020}`.
fn parse_disk_bal_chunk_dir_name(name: &str) -> Option<(BlockNumber, BlockNumber)> {
    let (start, end) = name.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Sealed, B256};
    use std::fs;
    use tokio_stream::StreamExt;

    fn sealed_bal(bal: Bytes) -> SealedBal {
        Sealed::new_unchecked(bal.clone(), keccak256(&bal))
    }

    fn tmp_disk_store(
        config: DiskBalStoreConfig,
    ) -> (DiskBalStore, tempfile::TempDir, DiskBalStoreConfig) {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBalStore::open(dir.path(), config).unwrap();
        (store, dir, config)
    }

    #[test]
    fn disk_store_reopens_and_looks_up_by_hash() {
        let config = DiskBalStoreConfig::unbounded().with_chunk_size(2);
        let (store, dir, _) = tmp_disk_store(config);
        let hash = B256::random();
        let bal = Bytes::from_static(b"disk-bal");

        store.insert(NumHash::new(1, hash), sealed_bal(bal.clone())).unwrap();
        store.flush().unwrap();
        drop(store);

        let reopened = DiskBalStore::open(dir.path(), config).unwrap();

        assert_eq!(reopened.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
    }

    #[test]
    fn disk_store_prunes_complete_old_chunks() {
        let config = DiskBalStoreConfig::with_retention(PruneMode::Distance(2)).with_chunk_size(2);
        let (store, dir, _) = tmp_disk_store(config);
        let old_hash = B256::random();
        let retained_hash = B256::random();
        let tip_hash = B256::random();
        let old_bal = Bytes::from_static(b"old");
        let retained_bal = Bytes::from_static(b"retained");
        let tip_bal = Bytes::from_static(b"tip");

        store.insert(NumHash::new(0, old_hash), sealed_bal(old_bal.clone())).unwrap();
        store.insert(NumHash::new(2, retained_hash), sealed_bal(retained_bal.clone())).unwrap();
        store.insert(NumHash::new(4, tip_hash), sealed_bal(tip_bal.clone())).unwrap();
        store.flush().unwrap();

        assert!(store.is_cached(&old_hash));
        assert!(store.is_cached(&retained_hash));
        assert!(store.is_cached(&tip_hash));
        assert_eq!(
            store.get_by_hashes(&[old_hash, retained_hash, tip_hash]).unwrap(),
            vec![Some(old_bal), Some(retained_bal.clone()), Some(tip_bal.clone())]
        );

        store.clear_cache();
        assert_eq!(
            store.get_by_hashes(&[old_hash, retained_hash, tip_hash]).unwrap(),
            vec![None, Some(retained_bal), Some(tip_bal)]
        );

        let root = dir.path().join(DISK_BAL_STORE_VERSION_DIR);
        assert!(!disk_bal_chunk_dir(&root, 0, config.chunk_size).exists());
        assert!(disk_bal_chunk_dir(&root, 2, config.chunk_size).exists());
    }

    #[test]
    fn disk_store_limited_lookup_returns_prefix() {
        let (store, _dir, _) = tmp_disk_store(DiskBalStoreConfig::unbounded().with_chunk_size(10));
        let hash0 = B256::random();
        let hash1 = B256::random();
        let hash2 = B256::random();
        let bal0 = Bytes::from_static(&[0xc1, 0x01]);
        let bal1 = Bytes::from_static(&[0xc1, 0x02]);
        let bal2 = Bytes::from_static(&[0xc1, 0x03]);

        store.insert(NumHash::new(1, hash0), sealed_bal(bal0.clone())).unwrap();
        store.insert(NumHash::new(2, hash1), sealed_bal(bal1.clone())).unwrap();
        store.insert(NumHash::new(3, hash2), sealed_bal(bal2)).unwrap();

        let limited = store
            .get_by_hashes_with_limit(
                &[hash0, hash1, hash2],
                GetBlockAccessListLimit::ResponseSizeSoftLimit(2),
            )
            .unwrap();

        assert_eq!(limited, vec![bal0, bal1]);
    }

    #[test]
    fn disk_store_range_lookup_stops_at_gap() {
        let (store, _dir, _) = tmp_disk_store(DiskBalStoreConfig::unbounded().with_chunk_size(10));
        let hash1 = B256::random();
        let hash3 = B256::random();
        let bal1 = Bytes::from_static(b"one");

        store.insert(NumHash::new(1, hash1), sealed_bal(bal1.clone())).unwrap();
        store.insert(NumHash::new(3, hash3), sealed_bal(Bytes::from_static(b"three"))).unwrap();

        assert_eq!(store.get_by_range(1, 3).unwrap(), vec![bal1]);
    }

    #[test]
    fn disk_store_reopen_keeps_latest_duplicate_hash_entry() {
        let config = DiskBalStoreConfig::unbounded().with_chunk_size(10);
        let (store, dir, _) = tmp_disk_store(config);
        let hash = B256::random();
        let latest = Bytes::from_static(b"latest");

        store.insert(NumHash::new(1, hash), sealed_bal(Bytes::from_static(b"old"))).unwrap();
        store.insert(NumHash::new(2, hash), sealed_bal(latest.clone())).unwrap();
        store.flush().unwrap();
        drop(store);

        let reopened = DiskBalStore::open(dir.path(), config).unwrap();

        assert_eq!(reopened.get_by_hashes(&[hash]).unwrap(), vec![Some(latest)]);
        assert!(reopened.get_by_range(1, 2).unwrap().is_empty());
    }

    #[tokio::test]
    async fn disk_store_insert_notifies_subscribers() {
        let (store, _dir, _) = tmp_disk_store(DiskBalStoreConfig::unbounded().with_chunk_size(10));
        let hash = B256::random();
        let block_number = 7;
        let bal = sealed_bal(Bytes::from_static(b"bal"));
        let mut stream = store.bal_stream();

        store.insert(NumHash::new(block_number, hash), bal.clone()).unwrap();

        assert_eq!(
            stream.next().await.unwrap(),
            BalNotification::new(NumHash::new(block_number, hash), bal)
        );
    }

    #[test]
    fn disk_store_caches_inserted_bals_and_evicts_lru() {
        let (store, _dir, _) =
            tmp_disk_store(DiskBalStoreConfig::unbounded().with_max_cached_entries(2));
        let hash0 = B256::random();
        let hash1 = B256::random();
        let hash2 = B256::random();

        store.insert(NumHash::new(0, hash0), sealed_bal(Bytes::from_static(b"zero"))).unwrap();
        store.insert(NumHash::new(1, hash1), sealed_bal(Bytes::from_static(b"one"))).unwrap();
        store.insert(NumHash::new(2, hash2), sealed_bal(Bytes::from_static(b"two"))).unwrap();

        assert_eq!(store.cache_len(), 2);
        assert!(!store.is_cached(&hash0));
        assert!(store.is_cached(&hash1));
        assert!(store.is_cached(&hash2));
    }

    #[test]
    fn disk_store_reads_populate_cache() {
        let (store, _dir, _) =
            tmp_disk_store(DiskBalStoreConfig::unbounded().with_max_cached_entries(2));
        let hash = B256::random();
        let bal = Bytes::from_static(b"cached-after-read");

        store.insert(NumHash::new(0, hash), sealed_bal(bal.clone())).unwrap();
        store.flush().unwrap();
        store.clear_cache();

        assert!(!store.is_cached(&hash));
        assert_eq!(store.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
        assert!(store.is_cached(&hash));
    }

    #[test]
    fn disk_store_cache_hit_avoids_disk_read() {
        let config = DiskBalStoreConfig::unbounded().with_chunk_size(10).with_max_cached_entries(1);
        let (store, dir, _) = tmp_disk_store(config);
        let hash = B256::random();
        let bal = Bytes::from_static(b"cache-hit");

        store.insert(NumHash::new(0, hash), sealed_bal(bal.clone())).unwrap();
        store.flush().unwrap();

        let root = dir.path().join(DISK_BAL_STORE_VERSION_DIR);
        let data_path =
            disk_bal_chunk_dir(&root, 0, config.chunk_size).join(DISK_BAL_DATA_FILE_NAME);
        fs::remove_file(data_path).unwrap();

        assert_eq!(store.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
    }

    #[test]
    fn disk_store_zero_cache_size_disables_payload_cache() {
        let (store, _dir, _) =
            tmp_disk_store(DiskBalStoreConfig::unbounded().with_max_cached_entries(0));
        let hash = B256::random();
        let bal = Bytes::from_static(b"uncached");

        store.insert(NumHash::new(0, hash), sealed_bal(bal.clone())).unwrap();

        assert_eq!(store.cache_len(), 0);
        assert_eq!(store.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
        assert_eq!(store.cache_len(), 0);
    }

    #[test]
    fn disk_store_insert_defers_disk_io_until_flush() {
        let config = DiskBalStoreConfig::unbounded().with_chunk_size(10);
        let (store, dir, _) = tmp_disk_store(config);
        let hash = B256::random();
        let bal = Bytes::from_static(b"pending");

        store.insert(NumHash::new(0, hash), sealed_bal(bal.clone())).unwrap();

        let root = dir.path().join(DISK_BAL_STORE_VERSION_DIR);
        let chunk_dir = disk_bal_chunk_dir(&root, 0, config.chunk_size);
        assert!(!chunk_dir.exists());
        assert_eq!(store.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);

        store.flush().unwrap();

        assert!(chunk_dir.join(DISK_BAL_DATA_FILE_NAME).exists());
        assert!(chunk_dir.join(DISK_BAL_INDEX_FILE_NAME).exists());
    }

    #[test]
    fn disk_store_flush_persists_pending_bals() {
        let config = DiskBalStoreConfig::unbounded().with_chunk_size(10);
        let (store, dir, _) = tmp_disk_store(config);
        let hash = B256::random();
        let bal = Bytes::from_static(b"flushed");

        store.insert(NumHash::new(0, hash), sealed_bal(bal.clone())).unwrap();
        store.flush().unwrap();
        drop(store);

        let reopened = DiskBalStore::open(dir.path(), config).unwrap();

        assert_eq!(reopened.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
    }
}
