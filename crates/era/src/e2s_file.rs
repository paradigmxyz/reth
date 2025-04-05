//! `E2Store` file reader
//!
//! See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md>

use crate::e2s_types::{E2sError, Entry, Version};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};

/// A reader for `E2Store` files that wraps a [`BufReader`].

#[derive(Debug)]
pub struct E2StoreReader<R: Read> {
    /// Buffered reader
    reader: BufReader<R>,
}

impl<R: Read + Seek> E2StoreReader<R> {
    /// Create a new [`E2StoreReader`]
    pub fn new(reader: R) -> Self {
        Self { reader: BufReader::new(reader) }
    }

    /// Read and validate the version record
    pub fn read_version(&mut self) -> Result<Option<Entry>, E2sError> {
        // Reset reader to beginning
        self.reader.seek(SeekFrom::Start(0))?;

        match Entry::read(&mut self.reader)? {
            Some(entry) if entry.is_version() => Ok(Some(entry)),
            Some(_) => Err(E2sError::Ssz("First entry must be a Version entry".to_string())),
            None => Ok(None),
        }
    }

    /// Read the next entry from the file
    pub fn read_next_entry(&mut self) -> Result<Option<Entry>, E2sError> {
        Entry::read(&mut self.reader)
    }

    /// Iterate through all entries, including the version entry
    pub fn entries(&mut self) -> Result<Vec<Entry>, E2sError> {
        // Reset reader to beginning
        self.reader.seek(SeekFrom::Start(0))?;

        let mut entries = Vec::new();

        while let Some(entry) = self.read_next_entry()? {
            entries.push(entry);
        }

        Ok(entries)
    }
}

/// A writer for `E2Store` files that wraps a [`BufWriter`].
#[derive(Debug)]
pub struct E2StoreWriter<W: Write> {
    /// Buffered writer
    writer: BufWriter<W>,
    /// Tracks whether this writer has written a version entry
    has_written_version: bool,
}

impl<W: Write> E2StoreWriter<W> {
    /// Create a new [`E2StoreWriter`]
    pub fn new(writer: W) -> Self {
        Self { writer: BufWriter::new(writer), has_written_version: false }
    }

    /// Create a new [`E2StoreWriter`] and write the version entry
    pub fn with_version(writer: W) -> Result<Self, E2sError> {
        let mut writer = Self::new(writer);
        writer.write_version()?;
        Ok(writer)
    }

    /// Write the version entry as the first entry in the file.
    /// This must be called before writing any other entries.
    pub fn write_version(&mut self) -> Result<(), E2sError> {
        if self.has_written_version {
            return Ok(());
        }

        let version = Version;
        version.encode(&mut self.writer)?;
        self.has_written_version = true;
        Ok(())
    }

    /// Write an entry to the file.
    /// If a version entry has not been written yet, it will be added.
    pub fn write_entry(&mut self, entry: &Entry) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        entry.write(&mut self.writer)?;
        Ok(())
    }

    /// Flush any buffered data to the underlying writer
    pub fn flush(&mut self) -> Result<(), E2sError> {
        self.writer.flush().map_err(E2sError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2s_types::{BLOCK_INDEX, VERSION};
    use std::io::Cursor;

    /// Creates properly formatted [`BlockIndex`] data
    fn create_block_index_data(block_number: u64, offset: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&block_number.to_le_bytes());
        data.extend_from_slice(&offset.to_le_bytes());
        data
    }

    #[test]
    fn test_e2store_reader() -> Result<(), E2sError> {
        // Create a mock e2store file in memory
        let mut mock_file = Vec::new();

        let version_entry = Entry::new(VERSION, Vec::new());
        version_entry.write(&mut mock_file)?;

        let block_index_entry1 = Entry::new(BLOCK_INDEX, create_block_index_data(1, 1024));
        block_index_entry1.write(&mut mock_file)?;

        let block_index_entry2 = Entry::new(BLOCK_INDEX, create_block_index_data(2, 2048));
        block_index_entry2.write(&mut mock_file)?;

        let custom_type = [0x99, 0x99];
        let custom_entry = Entry::new(custom_type, vec![10, 11, 12]);
        custom_entry.write(&mut mock_file)?;

        let cursor = Cursor::new(mock_file);
        let mut e2store_reader = E2StoreReader::new(cursor);

        let version = e2store_reader.read_version()?;
        assert!(version.is_some());

        let entries = e2store_reader.entries()?;

        // Validate entries
        assert_eq!(entries.len(), 4);
        // First entry should be version
        assert!(entries[0].is_version());
        // Second entry should be block index
        assert!(entries[1].is_block_index());
        // Third entry is block index
        assert!(entries[2].is_block_index());
        // Fourth entry is custom type
        assert!(entries[3].entry_type == [0x99, 0x99]);

        Ok(())
    }

    #[test]
    fn test_empty_file() -> Result<(), E2sError> {
        // Create an empty file
        let mock_file = Vec::new();

        // Create reader
        let cursor = Cursor::new(mock_file);
        let mut e2store_reader = E2StoreReader::new(cursor);

        // Reading version should return None
        let version = e2store_reader.read_version()?;
        assert!(version.is_none());

        // Entries should be empty
        let entries = e2store_reader.entries()?;
        assert!(entries.is_empty());

        Ok(())
    }

    #[test]
    fn test_read_next_entry() -> Result<(), E2sError> {
        let mut mock_file = Vec::new();

        let version_entry = Entry::new(VERSION, Vec::new());
        version_entry.write(&mut mock_file)?;

        let block_entry = Entry::new(BLOCK_INDEX, create_block_index_data(1, 1024));
        block_entry.write(&mut mock_file)?;

        let cursor = Cursor::new(mock_file);
        let mut reader = E2StoreReader::new(cursor);

        let first = reader.read_next_entry()?.unwrap();
        assert!(first.is_version());

        let second = reader.read_next_entry()?.unwrap();
        assert!(second.is_block_index());

        let third = reader.read_next_entry()?;
        assert!(third.is_none());

        Ok(())
    }

    #[test]
    fn test_e2store_writer() -> Result<(), E2sError> {
        let mut buffer = Vec::new();

        {
            let mut writer = E2StoreWriter::new(&mut buffer);

            // Write version entry
            writer.write_version()?;

            // Write a block index entry
            let block_entry = Entry::new(BLOCK_INDEX, create_block_index_data(1, 1024));
            writer.write_entry(&block_entry)?;

            // Write a custom entry
            let custom_type = [0x99, 0x99];
            let custom_entry = Entry::new(custom_type, vec![10, 11, 12]);
            writer.write_entry(&custom_entry)?;

            writer.flush()?;
        }

        let cursor = Cursor::new(&buffer);
        let mut reader = E2StoreReader::new(cursor);

        let entries = reader.entries()?;
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_version());
        assert!(entries[1].is_block_index());
        assert_eq!(entries[2].entry_type, [0x99, 0x99]);

        Ok(())
    }

    #[test]
    fn test_writer_implicit_version_insertion() -> Result<(), E2sError> {
        let mut buffer = Vec::new();

        {
            // Writer without explicitly writing the version
            let mut writer = E2StoreWriter::new(&mut buffer);

            // Write an entry, it should automatically add a version first
            let custom_type = [0x42, 0x42];
            let custom_entry = Entry::new(custom_type, vec![1, 2, 3, 4]);
            writer.write_entry(&custom_entry)?;

            // Write another entry
            let another_custom = Entry::new([0x43, 0x43], vec![5, 6, 7, 8]);
            writer.write_entry(&another_custom)?;

            writer.flush()?;
        }

        let cursor = Cursor::new(&buffer);
        let mut reader = E2StoreReader::new(cursor);

        let version = reader.read_version()?;
        assert!(version.is_some(), "Version entry should have been auto-added");

        let entries = reader.entries()?;
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_version());
        assert_eq!(entries[1].entry_type, [0x42, 0x42]);
        assert_eq!(entries[1].data, vec![1, 2, 3, 4]);
        assert_eq!(entries[2].entry_type, [0x43, 0x43]);
        assert_eq!(entries[2].data, vec![5, 6, 7, 8]);

        Ok(())
    }

    #[test]
    fn test_writer_prevents_duplicate_versions() -> Result<(), E2sError> {
        let mut buffer = Vec::new();

        {
            let mut writer = E2StoreWriter::new(&mut buffer);

            // Call write_version multiple times, it should only write once
            writer.write_version()?;
            writer.write_version()?;
            writer.write_version()?;

            // Write an entry
            let block_entry = Entry::new(BLOCK_INDEX, create_block_index_data(42, 8192));
            writer.write_entry(&block_entry)?;

            writer.flush()?;
        }

        // Verify only one version entry was written
        let cursor = Cursor::new(&buffer);
        let mut reader = E2StoreReader::new(cursor);

        let entries = reader.entries()?;
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_version());
        assert!(entries[1].is_block_index());

        Ok(())
    }
}
