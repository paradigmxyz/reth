use crate::direct_file::{AlignedBuffer, DirectFile};
use schnellru::{ByLength, LruMap};
use std::{
    io,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
};

/// A direct-I/O reader with a bounded block cache shared across readers and cursors.
///
/// The file length and cached contents describe the file at opening. Callers must
/// reopen after changing existing bytes, including truncation. Each opening has a
/// unique identity, so a replacement file or a reused descriptor cannot hit stale
/// entries. Appending beyond this reader's length does not invalidate its contents.
///
/// All readers share a 64 MiB buffer budget (plus cache bookkeeping). Cache misses
/// read 4 KiB blocks, or the filesystem's alignment if larger, using direct I/O.
#[derive(Debug)]
pub struct CachedFile {
    file: DirectFile,
    id: u64,
    len: u64,
    block_size: usize,
    #[cfg(test)]
    reads: AtomicU64,
}

impl CachedFile {
    /// Opens an immutable view of a file, sharing cached blocks between its users.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let file = DirectFile::open(path, false, false)?;
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            len: file.metadata()?.len(),
            block_size: 4096.max(file.alignment()),
            file,
            #[cfg(test)]
            reads: AtomicU64::new(0),
        })
    }

    /// Returns the file length observed at opening.
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Returns whether the file was empty at opening.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reads a range through the shared cache, including ranges crossing blocks.
    pub fn read_exact_at(&self, mut output: &mut [u8], mut offset: u64) -> io::Result<()> {
        if offset.checked_add(output.len() as u64).is_none_or(|end| end > self.len) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        while !output.is_empty() {
            let index = offset / self.block_size as u64;
            let within = (offset % self.block_size as u64) as usize;
            let block = self.block(index)?;
            let count = output.len().min(self.block_size - within);
            output[..count].copy_from_slice(&block.as_slice()[within..within + count]);
            output = &mut output[count..];
            offset += count as u64;
        }
        Ok(())
    }

    fn block(&self, index: u64) -> io::Result<Arc<AlignedBuffer>> {
        let key = (self.id, index);
        let shard = &CACHE[((self.id.wrapping_mul(31) ^ index) as usize) % CACHE.len()];
        if let Some(block) = shard.lock().map_err(poisoned)?.blocks.get(&key) {
            return Ok(Arc::clone(block));
        }

        // Never hold a shared cache lock across disk I/O. Concurrent misses can
        // duplicate one read, then converge on the first inserted buffer below.
        let start = index * self.block_size as u64;
        let mut buffer = AlignedBuffer::new(self.block_size, self.file.alignment())?;
        let n = self.file.read_aligned_at(buffer.as_mut_slice(), start)?;
        if n < (self.len - start).min(self.block_size as u64) as usize {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        #[cfg(test)]
        self.reads.fetch_add(1, Ordering::Relaxed);
        let mut shard = shard.lock().map_err(poisoned)?;
        if let Some(block) = shard.blocks.get(&key) {
            return Ok(Arc::clone(block));
        }
        let block = Arc::new(buffer);
        shard.insert(key, Arc::clone(&block));
        Ok(block)
    }
}

/// Split locking and eviction into independent shards; budget includes alignment padding.
static CACHE: LazyLock<[Mutex<CacheShard>; 16]> =
    LazyLock::new(|| std::array::from_fn(|_| Mutex::new(CacheShard::new(4 * 1024 * 1024))));

#[derive(Debug)]
struct CacheShard {
    blocks: LruMap<(u64, u64), Arc<AlignedBuffer>>,
    bytes: usize,
    limit: usize,
}

impl CacheShard {
    fn new(limit: usize) -> Self {
        Self { blocks: LruMap::new(ByLength::new(u32::MAX)), bytes: 0, limit }
    }

    fn insert(&mut self, key: (u64, u64), block: Arc<AlignedBuffer>) {
        let size = block.allocated_bytes();
        if size > self.limit {
            return;
        }
        if let Some(old) = self.blocks.remove(&key) {
            self.bytes -= old.allocated_bytes();
        }
        while self.bytes + size > self.limit {
            let (_, oldest) = self.blocks.pop_oldest().expect("nonempty cache above budget");
            self.bytes -= oldest.allocated_bytes();
        }
        self.blocks.insert(key, block);
        self.bytes += size;
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> io::Error {
    io::Error::other("static file block cache lock poisoned")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reuses_blocks_and_handles_boundaries() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let expected: Vec<u8> = (0..20000).map(|i| (i % 251) as u8).collect();
        file.write_all(&expected).unwrap();
        let reader = CachedFile::open(file.path()).unwrap();
        let mut blocks = std::collections::BTreeSet::new();
        for range in [0..8, 1..10, 4090..4110, 12000..13000, 4090..4110, 19999..20000] {
            blocks.extend(range.start / reader.block_size..=(range.end - 1) / reader.block_size);
            let mut bytes = vec![0; range.len()];
            reader.read_exact_at(&mut bytes, range.start as u64).unwrap();
            assert_eq!(bytes, expected[range]);
        }
        // Each distinct block is read once, regardless of overlapping requests.
        assert_eq!(reader.reads.load(Ordering::Relaxed), blocks.len() as u64);
        assert!(reader.read_exact_at(&mut [0; 2], 19999).is_err());
        reader.read_exact_at(&mut [], 20000).unwrap();
    }

    #[test]
    fn reopening_after_truncation_does_not_reuse_cached_tail() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), vec![1; 5000]).unwrap();
        let old = CachedFile::open(file.path()).unwrap();
        let mut byte = [0];
        old.read_exact_at(&mut byte, 4500).unwrap();
        std::fs::write(file.path(), vec![2; 4800]).unwrap();
        let new = CachedFile::open(file.path()).unwrap();
        new.read_exact_at(&mut byte, 4500).unwrap();
        assert_eq!(byte, [2]);
        assert!(new.read_exact_at(&mut byte, 4800).is_err());
    }

    #[test]
    fn concurrent_readers_share_cached_blocks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), vec![7; 8192]).unwrap();
        let reader = CachedFile::open(file.path()).unwrap();
        reader.read_exact_at(&mut [0; 8192], 0).unwrap();
        let reads = reader.reads.load(Ordering::Relaxed);
        std::thread::scope(|scope| {
            for offset in [0, 12, 4090, 8100] {
                let reader = &reader;
                scope.spawn(move || {
                    for _ in 0..100 {
                        let mut bytes = [0; 20];
                        reader.read_exact_at(&mut bytes, offset).unwrap();
                        assert_eq!(bytes, [7; 20]);
                    }
                });
            }
        });
        assert_eq!(reader.reads.load(Ordering::Relaxed), reads);
    }

    #[test]
    fn cache_evicts_least_recent_blocks_within_byte_budget() {
        let block = || Arc::new(AlignedBuffer::new(4096, 4096).unwrap());
        let size = block().allocated_bytes();
        let mut cache = CacheShard::new(2 * size);
        cache.insert((0, 0), block());
        cache.insert((0, 1), block());
        cache.blocks.get(&(0, 0)).unwrap();
        cache.insert((0, 2), block());
        assert!(cache.blocks.peek(&(0, 1)).is_none());
        assert!(cache.blocks.peek(&(0, 0)).is_some());
        assert_eq!(cache.bytes, 2 * size);
        cache.insert((0, 0), block());
        assert_eq!(cache.bytes, 2 * size);
    }
}
