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

    /// Iterate through all entries, including the version entry
    pub fn entries(&mut self) -> Result<Vec<Entry>, E2sError> {
        // Reset reader to beginning
        self.reader.seek(SeekFrom::Start(0))?;

        let mut entries = Vec::new();

        while let Some(entry) = Entry::read(&mut self.reader)? {
            entries.push(entry);
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2s_types::{BLOCK_INDEX, VERSION};
    use std::io::Cursor;

    #[test]
    fn test_e2store_reader() -> Result<(), E2sError> {
        // Create a mock e2store file in memory
        let mut mock_file = Vec::new();

        // Write version entry
        let version_entry = Entry::new(VERSION, Vec::new());
        version_entry.write(&mut mock_file)?;

        // Write multiple entries of different types
        let block_index_entry1 = Entry::new(BLOCK_INDEX, vec![1, 2, 3, 4]);
        block_index_entry1.write(&mut mock_file)?;

        let block_index_entry2 = Entry::new(BLOCK_INDEX, vec![5, 6, 7, 8, 9]);
        block_index_entry2.write(&mut mock_file)?;

        let custom_type = [0x99, 0x99]; // Application-specific type
        let custom_entry = Entry::new(custom_type, vec![10, 11, 12]);
        custom_entry.write(&mut mock_file)?;

        // Create reader and process entries
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
}
