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
    ///
    /// The file is healed to match `committed_len` (from the segment header):
    /// - Partial records (from crash mid-write) are truncated to record boundary
    /// - Extra complete records (from crash after sidecar sync but before header commit) are
    ///   truncated to match the committed length
    /// - If the file has fewer records than committed, returns an error (data corruption)
    ///
    /// This mirrors `NippyJar`'s healing behavior where config/header is the commit boundary.
    pub fn new(path: impl AsRef<Path>, committed_len: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.as_ref())?;

        let file_len = file.metadata()?.len();
        let remainder = file_len % Self::RECORD_SIZE as u64;

        // First, truncate any partial record from crash mid-write
        let aligned_len = if remainder != 0 {
            let truncated_len = file_len - remainder;
            tracing::warn!(
                target: "reth::static_file",
                path = %path.as_ref().display(),
                original_len = file_len,
                truncated_len,
                "Truncating partial changeset offset record"
            );
            file.set_len(truncated_len)?;
            file.sync_all()?; // Sync required for crash safety
            truncated_len
        } else {
            file_len
        };

        let records_in_file = aligned_len / Self::RECORD_SIZE as u64;

        // Heal sidecar to match committed header length
        match records_in_file.cmp(&committed_len) {
            std::cmp::Ordering::Greater => {
                // Sidecar has uncommitted records from a crash - truncate them
                let target_len = committed_len * Self::RECORD_SIZE as u64;
                tracing::warn!(
                    target: "reth::static_file",
                    path = %path.as_ref().display(),
                    sidecar_records = records_in_file,
                    committed_len,
                    "Truncating uncommitted changeset offset records after crash recovery"
                );
                file.set_len(target_len)?;
                file.sync_all()?; // Sync required for crash safety
            }
            std::cmp::Ordering::Less => {
                // INVARIANT VIOLATION: This should be impossible if healing ran correctly.
                //
                // All code paths call `heal_changeset_sidecar()` before this function, which
                // validates the sidecar against NippyJar state and corrects the header to match
                // the actual file size. Therefore, `committed_len` should always equal or exceed
                // `records_in_file` when this function is called.
                //
                // If we reach this error, it indicates:
                // - A bug in the healing logic (header not corrected properly)
                // - This function was called directly without going through healing
                // - External corruption occurred between healing and opening (extremely unlikely)
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "INVARIANT VIOLATION: Changeset offset sidecar has {} records but header expects {} \
                         (healing should have prevented this - possible bug in healing logic): {}",
                        records_in_file,
                        committed_len,
                        path.as_ref().display()
                    ),
                ));
            }
            std::cmp::Ordering::Equal => {}
        }

        let records_written = committed_len;
        let file = OpenOptions::new().create(true).append(true).open(path)?;

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

    /// Truncates the file to contain exactly `len` records and syncs to disk.
    /// Used after prune operations to reclaim space.
    ///
    /// The sync is required for crash safety - without it, a crash could
    /// resurrect the old file length.
    pub fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len * Self::RECORD_SIZE as u64)?;
        self.file.sync_all()?;
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

    /// Opens the changeset offset file for reading with an explicit length.
    ///
    /// The `len` parameter (from header metadata) bounds the reader - any records
    /// beyond this length are ignored. This ensures we only read committed data.
    pub fn new(path: impl AsRef<Path>, len: u64) -> io::Result<Self> {
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

        // Write (new file, committed_len=0)
        {
            let mut writer = ChangesetOffsetWriter::new(&path, 0).unwrap();
            writer.append(&ChangesetOffset::new(0, 5)).unwrap();
            writer.append(&ChangesetOffset::new(5, 3)).unwrap();
            writer.append(&ChangesetOffset::new(8, 10)).unwrap();
            writer.sync().unwrap();
            assert_eq!(writer.len(), 3);
        }

        // Read
        {
            let mut reader = ChangesetOffsetReader::new(&path, 3).unwrap();
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

        let mut writer = ChangesetOffsetWriter::new(&path, 0).unwrap();
        writer.append(&ChangesetOffset::new(0, 1)).unwrap();
        writer.append(&ChangesetOffset::new(1, 2)).unwrap();
        writer.append(&ChangesetOffset::new(3, 3)).unwrap();
        writer.sync().unwrap();

        writer.truncate(2).unwrap();
        assert_eq!(writer.len(), 2);

        let mut reader = ChangesetOffsetReader::new(&path, 2).unwrap();
        assert_eq!(reader.len(), 2);
        assert!(reader.get(2).unwrap().is_none());
    }

    #[test]
    fn test_partial_record_recovery() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.csoff");

        // Write 1 full record (16 bytes) + 8 trailing bytes (partial record)
        {
            let mut file = std::fs::File::create(&path).unwrap();
            // Full record: offset=100, num_changes=5
            file.write_all(&100u64.to_le_bytes()).unwrap();
            file.write_all(&5u64.to_le_bytes()).unwrap();
            // Partial record: only 8 bytes (incomplete)
            file.write_all(&200u64.to_le_bytes()).unwrap();
            file.sync_all().unwrap();
        }

        // Verify file has 24 bytes before opening with writer
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 24);

        // Open with writer, committed_len=1 (header committed 1 record)
        // Should truncate partial record and match committed length
        let writer = ChangesetOffsetWriter::new(&path, 1).unwrap();
        assert_eq!(writer.len(), 1);

        // Verify file was truncated to 16 bytes
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 16);

        // Verify the complete record is readable
        let mut reader = ChangesetOffsetReader::new(&path, 1).unwrap();
        assert_eq!(reader.len(), 1);
        let entry = reader.get(0).unwrap().unwrap();
        assert_eq!(entry.offset(), 100);
        assert_eq!(entry.num_changes(), 5);
    }

    #[test]
    fn test_len_bounds_reads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.csoff");

        // Write 3 records
        {
            let mut writer = ChangesetOffsetWriter::new(&path, 0).unwrap();
            writer.append(&ChangesetOffset::new(0, 10)).unwrap();
            writer.append(&ChangesetOffset::new(10, 20)).unwrap();
            writer.append(&ChangesetOffset::new(30, 30)).unwrap();
            writer.sync().unwrap();
            assert_eq!(writer.len(), 3);
        }

        // Open with len=2, ignoring the 3rd record
        let mut reader = ChangesetOffsetReader::new(&path, 2).unwrap();
        assert_eq!(reader.len(), 2);

        // First two records should be readable
        let entry0 = reader.get(0).unwrap().unwrap();
        assert_eq!(entry0.offset(), 0);
        assert_eq!(entry0.num_changes(), 10);

        let entry1 = reader.get(1).unwrap().unwrap();
        assert_eq!(entry1.offset(), 10);
        assert_eq!(entry1.num_changes(), 20);

        // Third record should be out of bounds (due to len=2)
        assert!(reader.get(2).unwrap().is_none());

        // get_range should also respect the len bound
        let range = reader.get_range(0, 5).unwrap();
        assert_eq!(range.len(), 2);
    }

    #[test]
    fn test_truncate_uncommitted_records_on_open() {
        // Simulates crash recovery where sidecar has more records than committed header length.
        // ChangesetOffsetWriter::new() should automatically truncate to committed_len.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.csoff");

        // Simulate: wrote 3 records, synced sidecar, but header only committed len=2
        {
            let mut writer = ChangesetOffsetWriter::new(&path, 0).unwrap();
            writer.append(&ChangesetOffset::new(0, 5)).unwrap();
            writer.append(&ChangesetOffset::new(5, 10)).unwrap();
            writer.append(&ChangesetOffset::new(15, 7)).unwrap(); // uncommitted
            writer.sync().unwrap();
            assert_eq!(writer.len(), 3);
        }

        // On "restart", new() heals by truncating to committed length
        let committed_len = 2u64;
        {
            let writer = ChangesetOffsetWriter::new(&path, committed_len).unwrap();
            assert_eq!(writer.len(), 2); // Healed to committed length
        }

        // Verify file is now correct length and new appends go to the right place
        {
            let mut writer = ChangesetOffsetWriter::new(&path, 2).unwrap();
            assert_eq!(writer.len(), 2);

            // Append a new record - should be at index 2, not index 3
            writer.append(&ChangesetOffset::new(15, 20)).unwrap();
            writer.sync().unwrap();
            assert_eq!(writer.len(), 3);
        }

        // Verify the records are correct
        {
            let mut reader = ChangesetOffsetReader::new(&path, 3).unwrap();
            assert_eq!(reader.len(), 3);

            let entry0 = reader.get(0).unwrap().unwrap();
            assert_eq!(entry0.offset(), 0);
            assert_eq!(entry0.num_changes(), 5);

            let entry1 = reader.get(1).unwrap().unwrap();
            assert_eq!(entry1.offset(), 5);
            assert_eq!(entry1.num_changes(), 10);

            // This should be the NEW record, not the old uncommitted one
            let entry2 = reader.get(2).unwrap().unwrap();
            assert_eq!(entry2.offset(), 15);
            assert_eq!(entry2.num_changes(), 20); // Not 7 from the old uncommitted record
        }
    }

    #[test]
    fn test_sidecar_shorter_than_committed_errors() {
        // If sidecar has fewer records than committed, it's data corruption - should error.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.csoff");

        // Write 1 record
        {
            let mut writer = ChangesetOffsetWriter::new(&path, 0).unwrap();
            writer.append(&ChangesetOffset::new(0, 5)).unwrap();
            writer.sync().unwrap();
        }

        // Try to open with committed_len=3 (header claims more than file has)
        let result = ChangesetOffsetWriter::new(&path, 3);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
