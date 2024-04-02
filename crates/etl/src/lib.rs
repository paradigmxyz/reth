//! ETL data collector.
//!
//! This crate is useful for dumping unsorted data into temporary files and iterating on their
//! sorted representation later on.
//!
//! This has multiple uses, such as optimizing database inserts (for Btree based databases) and
//! memory management (as it moves the buffer to disk instead of memory).

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use rayon::prelude::*;
use reth_db::table::{Compress, Encode, Key, Value};
use tempfile::{NamedTempFile, TempDir};

/// An ETL (extract, transform, load) data collector.
///
/// Data is pushed (extract) to the collector which internally flushes the data in a sorted
/// (transform) manner to files of some specified capacity. the data can later be iterated over
/// (load) in a sorted manner.
///
/// Used mainly to insert data into `MDBX` in a sorted manner. This is important because performance
/// and storage space degrades greatly if the data is inserted unsorted (eg. tables with hashes as
/// keys.) as opposed to append & sorted insert. Some benchmarks can be found [here](https://github.com/paradigmxyz/reth/pull/1130#issuecomment-1418642755).
#[derive(Debug)]
pub struct Collector<K, V>
where
    K: Encode + Ord,
    V: Compress,
{
    /// Parent directory where to create ETL files
    parent_dir: Option<PathBuf>,
    /// Directory for temporary file storage
    dir: Option<TempDir>,
    /// Collection of temporary ETL files
    files: Vec<EtlFile>,
    /// Current buffer size in bytes
    buffer_size_bytes: usize,
    /// Maximum buffer capacity in bytes, triggers flush when reached
    buffer_capacity_bytes: usize,
    /// In-memory buffer storing encoded and compressed key-value pairs
    buffer: Vec<(<K as Encode>::Encoded, <V as Compress>::Compressed)>,
    /// Total number of elements in the collector, including all files
    len: usize,
}

impl<K, V> Collector<K, V>
where
    K: Key,
    V: Value,
{
    /// Create a new collector with some capacity.
    ///
    /// Once the capacity (in bytes) is reached, the data is sorted and flushed to disk.
    pub fn new(buffer_capacity_bytes: usize, parent_dir: Option<PathBuf>) -> Self {
        Self {
            parent_dir,
            dir: None,
            buffer_size_bytes: 0,
            files: Vec::new(),
            buffer_capacity_bytes,
            buffer: Vec::new(),
            len: 0,
        }
    }

    /// Returns number of elements currently in the collector.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if there are currently no elements in the collector.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Clears the collector, removing all data, including the temporary directory.
    pub fn clear(&mut self) {
        self.dir = None;
        // Clear vectors and free the allocated memory
        self.files = Vec::new();
        self.buffer = Vec::new();
        self.buffer_size_bytes = 0;
        self.len = 0;
    }

    /// Insert an entry into the collector.
    pub fn insert(&mut self, key: K, value: V) -> io::Result<()> {
        let key = key.encode();
        let value = value.compress();
        self.buffer_size_bytes += key.as_ref().len() + value.as_ref().len();
        self.buffer.push((key, value));
        if self.buffer_size_bytes > self.buffer_capacity_bytes {
            self.flush()?;
        }
        self.len += 1;

        Ok(())
    }

    /// Returns a reference to the temporary directory used by the collector. If the directory
    /// doesn't exist, it will be created.
    fn dir(&mut self) -> io::Result<&TempDir> {
        if self.dir.is_none() {
            self.dir = match &self.parent_dir {
                Some(dir) => {
                    if !dir.exists() {
                        std::fs::create_dir_all(dir)?;
                    }
                    Some(TempDir::new_in(dir)?)
                }
                None => Some(TempDir::new()?),
            };
        }
        Ok(self.dir.as_ref().unwrap())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.buffer_size_bytes = 0;
        self.buffer.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut buf = Vec::with_capacity(self.buffer.len());
        std::mem::swap(&mut buf, &mut self.buffer);

        let path = self.dir()?.path().to_path_buf();
        self.files.push(EtlFile::new(path.as_path(), buf)?);

        Ok(())
    }

    /// Returns an iterator over the collector data.
    ///
    /// The items of the iterator are sorted across all underlying files.
    ///
    /// # Note
    ///
    /// The keys and values have been pre-encoded, meaning they *SHOULD NOT* be encoded or
    /// compressed again.
    pub fn iter(&mut self) -> std::io::Result<EtlIter<'_>> {
        // Flush the remaining items to disk
        if self.buffer_size_bytes > 0 {
            self.flush()?;
        }

        let mut heap = BinaryHeap::new();
        for (current_id, file) in self.files.iter_mut().enumerate() {
            if let Some((current_key, current_value)) = file.read_next()? {
                heap.push((Reverse((current_key, current_value)), current_id));
            }
        }

        Ok(EtlIter { heap, files: &mut self.files })
    }
}

/// `EtlIter` is an iterator for traversing through sorted key-value pairs in a collection of ETL
/// files. These files are created using the [`Collector`] and contain data where keys are encoded
/// and values are compressed.
///
/// This iterator returns each key-value pair in ascending order based on the key.
/// It is particularly designed to efficiently handle large datasets by employing a binary heap for
/// managing the iteration order.
#[derive(Debug)]
pub struct EtlIter<'a> {
    /// Heap managing the next items to be iterated.
    #[allow(clippy::type_complexity)]
    heap: BinaryHeap<(Reverse<(Vec<u8>, Vec<u8>)>, usize)>,
    /// Reference to the vector of ETL files being iterated over.
    files: &'a mut Vec<EtlFile>,
}

impl<'a> EtlIter<'a> {
    /// Peeks into the next element
    pub fn peek(&self) -> Option<&(Vec<u8>, Vec<u8>)> {
        self.heap.peek().map(|(Reverse(entry), _)| entry)
    }
}

impl<'a> Iterator for EtlIter<'a> {
    type Item = std::io::Result<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        // Get the next sorted entry from the heap
        let (Reverse(entry), id) = self.heap.pop()?;

        // Populate the heap with the next entry from the same file
        match self.files[id].read_next() {
            Ok(Some((key, value))) => {
                self.heap.push((Reverse((key, value)), id));
                Some(Ok(entry))
            }
            Ok(None) => Some(Ok(entry)),
            err => err.transpose(),
        }
    }
}

/// A temporary ETL file.
#[derive(Debug)]
struct EtlFile {
    file: BufReader<NamedTempFile>,
    len: usize,
}

impl EtlFile {
    /// Create a new file with the given data (which should be pre-sorted) at the given path.
    ///
    /// The file will be a temporary file.
    pub(crate) fn new<K, V>(dir: &Path, buffer: Vec<(K, V)>) -> std::io::Result<Self>
    where
        Self: Sized,
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let file = NamedTempFile::new_in(dir)?;
        let mut w = BufWriter::new(file);
        for entry in &buffer {
            let k = entry.0.as_ref();
            let v = entry.1.as_ref();

            w.write_all(&k.len().to_be_bytes())?;
            w.write_all(&v.len().to_be_bytes())?;
            w.write_all(k)?;
            w.write_all(v)?;
        }

        let mut file = BufReader::new(w.into_inner()?);
        file.seek(SeekFrom::Start(0))?;
        let len = buffer.len();
        Ok(Self { file, len })
    }

    /// Read the next entry in the file.
    ///
    /// Can return error if it reaches EOF before filling the internal buffers.
    pub(crate) fn read_next(&mut self) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
        if self.len == 0 {
            return Ok(None)
        }

        let mut buffer_key_length = [0; 8];
        let mut buffer_value_length = [0; 8];

        self.file.read_exact(&mut buffer_key_length)?;
        self.file.read_exact(&mut buffer_value_length)?;

        let key_length = usize::from_be_bytes(buffer_key_length);
        let value_length = usize::from_be_bytes(buffer_value_length);
        let mut key = vec![0; key_length];
        let mut value = vec![0; value_length];

        self.file.read_exact(&mut key)?;
        self.file.read_exact(&mut value)?;

        self.len -= 1;

        Ok(Some((key, value)))
    }
}

#[cfg(test)]
mod tests {
    use reth_primitives::{TxHash, TxNumber};

    use super::*;

    #[test]
    fn etl_hashes() {
        let mut entries: Vec<_> =
            (0..10_000).map(|id| (TxHash::random(), id as TxNumber)).collect();

        let mut collector = Collector::new(1024, None);
        assert!(collector.dir.is_none());

        for (k, v) in entries.clone() {
            collector.insert(k, v).unwrap();
        }
        entries.sort_unstable_by_key(|entry| entry.0);

        for (id, entry) in collector.iter().unwrap().enumerate() {
            let expected = entries[id];
            assert_eq!(
                entry.unwrap(),
                (expected.0.encode().to_vec(), expected.1.compress().to_vec())
            );
        }

        let temp_dir_path = collector.dir.as_ref().unwrap().path().to_path_buf();

        collector.clear();
        assert!(collector.dir.is_none());
        assert!(collector.files.is_empty());
        assert_eq!(collector.buffer_size_bytes, 0);
        assert!(collector.buffer.is_empty());
        assert_eq!(collector.len, 0);
        assert!(collector.is_empty());
        assert!(!temp_dir_path.exists());
    }
}
