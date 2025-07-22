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
    use crate::e2s_types::{SLOT_INDEX, VERSION};
    use std::io::Cursor;

    fn create_slot_index_data(starting_slot: u64, offsets: &[i64]) -> Vec<u8> {
        // Format: starting-slot | index | index | index ... | count
        let mut data = Vec::with_capacity(8 + offsets.len() * 8 + 8);

        // Add starting slot
        data.extend_from_slice(&starting_slot.to_le_bytes());

        // Add all offsets
        for offset in offsets {
            data.extend_from_slice(&offset.to_le_bytes());
        }

        // Add count
        data.extend_from_slice(&(offsets.len() as i64).to_le_bytes());

        data
    }

    #[test]
    fn test_e2store_reader() -> Result<(), E2sError> {
        // Create a mock e2store file in memory
        let mut mock_file = Vec::new();

        let version_entry = Entry::new(VERSION, Vec::new());
        version_entry.write(&mut mock_file)?;

        let slot_index_entry1 = Entry::new(SLOT_INDEX, create_slot_index_data(1, &[1024]));
        slot_index_entry1.write(&mut mock_file)?;

        let slot_index_entry2 = Entry::new(SLOT_INDEX, create_slot_index_data(2, &[2048]));
        slot_index_entry2.write(&mut mock_file)?;

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
        // Second entry should be slot index
        assert!(entries[1].is_slot_index());
        // Third entry is slot index
        assert!(entries[2].is_slot_index());
        // Fourth entry is custom type
        assert!(entries[3].entry_type == [0x99, 0x99]);

        Ok(())
    }

    #[test]
    fn test_slot_index_with_multiple_offsets() -> Result<(), E2sError> {
        let starting_slot = 100;
        let offsets = &[1024, 2048, 0, 0, 3072, 0, 4096];

        let slot_index_data = create_slot_index_data(starting_slot, offsets);

        let slot_index_entry = Entry::new(SLOT_INDEX, slot_index_data.clone());

        // Verify the slot index data format
        assert_eq!(slot_index_data.len(), 8 + offsets.len() * 8 + 8);

        // Check the starting slot
        let mut starting_slot_bytes = [0u8; 8];
        starting_slot_bytes.copy_from_slice(&slot_index_data[0..8]);
        assert_eq!(u64::from_le_bytes(starting_slot_bytes), starting_slot);

        // Check the count at the end
        let mut count_bytes = [0u8; 8];
        count_bytes.copy_from_slice(&slot_index_data[slot_index_data.len() - 8..]);
        assert_eq!(i64::from_le_bytes(count_bytes), offsets.len() as i64);

        // Verify we can write and read it back
        let mut buffer = Vec::new();
        slot_index_entry.write(&mut buffer)?;

        let cursor = Cursor::new(buffer);
        let mut reader = E2StoreReader::new(cursor);

        let read_entry = reader.read_next_entry()?.unwrap();
        assert!(read_entry.is_slot_index());

        assert_eq!(read_entry.data.len(), slot_index_data.len());

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

        let slot_entry = Entry::new(SLOT_INDEX, create_slot_index_data(1, &[1024]));
        slot_entry.write(&mut mock_file)?;

        let cursor = Cursor::new(mock_file);
        let mut reader = E2StoreReader::new(cursor);

        let first = reader.read_next_entry()?.unwrap();
        assert!(first.is_version());

        let second = reader.read_next_entry()?.unwrap();
        assert!(second.is_slot_index());

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
            let block_entry = Entry::new(SLOT_INDEX, create_slot_index_data(1, &[1024]));
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
        assert!(entries[1].is_slot_index());
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
            let block_entry = Entry::new(SLOT_INDEX, create_slot_index_data(42, &[8192]));
            writer.write_entry(&block_entry)?;

            writer.flush()?;
        }

        // Verify only one version entry was written
        let cursor = Cursor::new(&buffer);
        let mut reader = E2StoreReader::new(cursor);

        let entries = reader.entries()?;
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_version());
        assert!(entries[1].is_slot_index());

        Ok(())
    }

    #[test]
    fn test_e2store_multiple_roundtrip_conversions() -> Result<(), E2sError> {
        // Initial set of entries to test with varied types and sizes
        let entry1 = Entry::new([0x01, 0x01], vec![1, 2, 3, 4, 5]);
        let entry2 = Entry::new([0x02, 0x02], vec![10, 20, 30, 40, 50]);
        let entry3 = Entry::new(SLOT_INDEX, create_slot_index_data(123, &[45678]));
        let entry4 = Entry::new([0xFF, 0xFF], Vec::new());

        println!("Initial entries count: 4");

        // First write cycle : create initial buffer
        let mut buffer1 = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer1);
            writer.write_version()?;
            writer.write_entry(&entry1)?;
            writer.write_entry(&entry2)?;
            writer.write_entry(&entry3)?;
            writer.write_entry(&entry4)?;
            writer.flush()?;
        }

        // First read cycle : read from initial buffer
        let mut reader1 = E2StoreReader::new(Cursor::new(&buffer1));
        let entries1 = reader1.entries()?;

        println!("First read entries:");
        for (i, entry) in entries1.iter().enumerate() {
            println!("Entry {}: type {:?}, data len {}", i, entry.entry_type, entry.data.len());
        }
        println!("First read entries count: {}", entries1.len());

        // Verify first read content
        assert_eq!(entries1.len(), 5, "Should have 5 entries (version + 4 data)");
        assert!(entries1[0].is_version());
        assert_eq!(entries1[1].entry_type, [0x01, 0x01]);
        assert_eq!(entries1[1].data, vec![1, 2, 3, 4, 5]);
        assert_eq!(entries1[2].entry_type, [0x02, 0x02]);
        assert_eq!(entries1[2].data, vec![10, 20, 30, 40, 50]);
        assert!(entries1[3].is_slot_index());
        assert_eq!(entries1[4].entry_type, [0xFF, 0xFF]);
        assert_eq!(entries1[4].data.len(), 0);

        // Second write cycle : write what we just read
        let mut buffer2 = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer2);
            // Only write version once
            writer.write_version()?;

            // Skip the first entry ie the version since we already wrote it
            for entry in &entries1[1..] {
                writer.write_entry(entry)?;
            }
            writer.flush()?;
        }

        // Second read cycle - read the second buffer
        let mut reader2 = E2StoreReader::new(Cursor::new(&buffer2));
        let entries2 = reader2.entries()?;

        // Verify second read matches first read
        assert_eq!(entries1.len(), entries2.len());
        for i in 0..entries1.len() {
            assert_eq!(entries1[i].entry_type, entries2[i].entry_type);
            assert_eq!(entries1[i].data, entries2[i].data);
        }

        Ok(())
    }
}
