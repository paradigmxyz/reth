//! Changeset offset sidecar file I/O.
//!
//! Provides append-only writing and O(1) random-access reading for changeset offsets.
//! The file format is fixed-width 16-byte records: `[offset: u64 LE][num_changes: u64 LE]`.

use crate::ChangesetOffset;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Writer for appending changeset offsets to a sidecar file.
#[derive(Debug)]
pub struct ChangesetOffsetWriter {
    file: File,
    /// Number of records written.
    records_written: u64,
}

impl ChangesetOffsetWriter {
    /// Record size in bytes.
    const RECORD_SIZE: usize = 16;

    /// Opens or creates the changeset offset file for appending.
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;

        let records_written = file.metadata()?.len() / Self::RECORD_SIZE as u64;

        Ok(Self { file, records_written })
    }

    /// Appends a single changeset offset record.
    pub fn append(&mut self, offset: &ChangesetOffset) -> io::Result<()> {
        let mut buf = [0u8; Self::RECORD_SIZE];
        buf[..8].copy_from_slice(&offset.offset().to_le_bytes());
        buf[8..].copy_from_slice(&offset.num_changes().to_le_bytes());
        self.file.write_all(&buf)?;
        self.records_written += 1;
        Ok(())
    }

    /// Appends multiple changeset offset records.
    pub fn append_many(&mut self, offsets: &[ChangesetOffset]) -> io::Result<()> {
        for offset in offsets {
            self.append(offset)?;
        }
        Ok(())
    }

    /// Syncs all data to disk. Must be called before committing the header.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// Truncates the file to contain exactly `len` records.
    /// Used after prune operations to reclaim space.
    pub fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len * Self::RECORD_SIZE as u64)?;
        self.records_written = len;
        Ok(())
    }

    /// Returns the number of records in the file.
    pub const fn len(&self) -> u64 {
        self.records_written
    }

    /// Returns true if the file is empty.
    pub const fn is_empty(&self) -> bool {
        self.records_written == 0
    }
}

/// Reader for changeset offsets with O(1) random access.
#[derive(Debug)]
pub struct ChangesetOffsetReader {
    file: File,
    /// Cached file length in records.
    len: u64,
}

impl ChangesetOffsetReader {
    /// Record size in bytes.
    const RECORD_SIZE: usize = 16;

    /// Opens the changeset offset file for reading.
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len() / Self::RECORD_SIZE as u64;
        Ok(Self { file, len })
    }

    /// Opens with an explicit length (from header metadata).
    /// Any records beyond `len` are ignored.
    pub fn with_len(path: impl AsRef<Path>, len: u64) -> io::Result<Self> {
        let file = File::open(path)?;
        Ok(Self { file, len })
    }

    /// Reads a single changeset offset by block index.
    /// Returns None if index is out of bounds.
    pub fn get(&mut self, block_index: u64) -> io::Result<Option<ChangesetOffset>> {
        if block_index >= self.len {
            return Ok(None);
        }

        let byte_pos = block_index * Self::RECORD_SIZE as u64;
        self.file.seek(SeekFrom::Start(byte_pos))?;

        let mut buf = [0u8; Self::RECORD_SIZE];
        self.file.read_exact(&mut buf)?;

        let offset = u64::from_le_bytes(buf[..8].try_into().unwrap());
        let num_changes = u64::from_le_bytes(buf[8..].try_into().unwrap());

        Ok(Some(ChangesetOffset::new(offset, num_changes)))
    }

    /// Reads a range of changeset offsets.
    pub fn get_range(&mut self, start: u64, end: u64) -> io::Result<Vec<ChangesetOffset>> {
        let end = end.min(self.len);
        if start >= end {
            return Ok(Vec::new());
        }

        let count = (end - start) as usize;
        let byte_pos = start * Self::RECORD_SIZE as u64;
        self.file.seek(SeekFrom::Start(byte_pos))?;

        let mut result = Vec::with_capacity(count);
        let mut buf = [0u8; Self::RECORD_SIZE];

        for _ in 0..count {
            self.file.read_exact(&mut buf)?;
            let offset = u64::from_le_bytes(buf[..8].try_into().unwrap());
            let num_changes = u64::from_le_bytes(buf[8..].try_into().unwrap());
            result.push(ChangesetOffset::new(offset, num_changes));
        }

        Ok(result)
    }

    /// Returns the number of valid records.
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Returns true if there are no records.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_write_and_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.csoff");

        // Write
        {
            let mut writer = ChangesetOffsetWriter::new(&path).unwrap();
            writer.append(&ChangesetOffset::new(0, 5)).unwrap();
            writer.append(&ChangesetOffset::new(5, 3)).unwrap();
            writer.append(&ChangesetOffset::new(8, 10)).unwrap();
            writer.sync().unwrap();
            assert_eq!(writer.len(), 3);
        }

        // Read
        {
            let mut reader = ChangesetOffsetReader::new(&path).unwrap();
            assert_eq!(reader.len(), 3);

            let entry = reader.get(0).unwrap().unwrap();
            assert_eq!(entry.offset(), 0);
            assert_eq!(entry.num_changes(), 5);

            let entry = reader.get(1).unwrap().unwrap();
            assert_eq!(entry.offset(), 5);
            assert_eq!(entry.num_changes(), 3);

            let entry = reader.get(2).unwrap().unwrap();
            assert_eq!(entry.offset(), 8);
            assert_eq!(entry.num_changes(), 10);

            assert!(reader.get(3).unwrap().is_none());
        }
    }

    #[test]
    fn test_truncate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.csoff");

        let mut writer = ChangesetOffsetWriter::new(&path).unwrap();
        writer.append(&ChangesetOffset::new(0, 1)).unwrap();
        writer.append(&ChangesetOffset::new(1, 2)).unwrap();
        writer.append(&ChangesetOffset::new(3, 3)).unwrap();
        writer.sync().unwrap();

        writer.truncate(2).unwrap();
        assert_eq!(writer.len(), 2);

        let mut reader = ChangesetOffsetReader::new(&path).unwrap();
        assert_eq!(reader.len(), 2);
        assert!(reader.get(2).unwrap().is_none());
    }
}
