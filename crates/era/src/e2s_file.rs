//! `E2Store` file reader
//!
//! See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md>

use crate::e2s_types::{E2sError, Entry};
use std::io::{BufReader, Read, Seek, SeekFrom};

/// A reader for `E2Store` files that wraps a [`BufReader`]
#[derive(Debug)]
pub struct E2StoreReader<R: Read> {
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
}
