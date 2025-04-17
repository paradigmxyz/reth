//! Represents a complete Era1 file
//!
//! The structure of an Era1 file follows the specification:
//! `Version | block-tuple* | other-entries* | Accumulator | BlockIndex`
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

use crate::{
    e2s_file::{E2StoreReader, E2StoreWriter},
    e2s_types::{E2sError, Entry, Version},
    era1_types::{BlockIndex, Era1Group, Era1Id, BLOCK_INDEX},
    execution_types::{
        self, Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts,
        TotalDifficulty, MAX_BLOCKS_PER_ERA1,
    },
};
use alloy_primitives::BlockNumber;
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

/// Era1 file interface
#[derive(Debug)]
pub struct Era1File {
    /// Version record, must be the first record in the file
    pub version: Version,

    /// Main content group of the Era1 file
    pub group: Era1Group,

    /// File identifier
    pub id: Era1Id,
}

impl Era1File {
    /// Create a new [`Era1File`]
    pub const fn new(group: Era1Group, id: Era1Id) -> Self {
        Self { version: Version, group, id }
    }

    /// Get a block by its number, if present in this file
    pub fn get_block_by_number(&self, number: BlockNumber) -> Option<&BlockTuple> {
        let index = (number - self.group.block_index.starting_number) as usize;
        (index < self.group.blocks.len()).then(|| &self.group.blocks[index])
    }

    /// Get the range of block numbers contained in this file
    pub fn block_range(&self) -> std::ops::RangeInclusive<BlockNumber> {
        let start = self.group.block_index.starting_number;
        let end = start + (self.group.blocks.len() as u64) - 1;
        start..=end
    }

    /// Check if this file contains a specific block number
    pub fn contains_block(&self, number: BlockNumber) -> bool {
        self.block_range().contains(&number)
    }
}
/// Reader for Era1 files that builds on top of [`E2StoreReader`]
#[derive(Debug)]
pub struct Era1Reader<R: Read> {
    reader: E2StoreReader<R>,
}

/// An iterator of [`BlockTuple`] streaming from [`E2StoreReader`].
#[derive(Debug)]
pub struct BlockTupleIterator<'r, R: Read> {
    reader: &'r mut E2StoreReader<R>,
    headers: VecDeque<CompressedHeader>,
    bodies: VecDeque<CompressedBody>,
    receipts: VecDeque<CompressedReceipts>,
    difficulties: VecDeque<TotalDifficulty>,
    other_entries: Vec<Entry>,
    accumulator: Option<Accumulator>,
    block_index: Option<BlockIndex>,
}

impl<'r, R: Read> BlockTupleIterator<'r, R> {
    fn new(reader: &'r mut E2StoreReader<R>) -> Self {
        Self {
            reader,
            headers: Default::default(),
            bodies: Default::default(),
            receipts: Default::default(),
            difficulties: Default::default(),
            other_entries: Default::default(),
            accumulator: None,
            block_index: None,
        }
    }
}

impl<'r, R: Read + Seek> Iterator for BlockTupleIterator<'r, R> {
    type Item = Result<BlockTuple, E2sError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_result().transpose()
    }
}

impl<'r, R: Read + Seek> BlockTupleIterator<'r, R> {
    fn next_result(&mut self) -> Result<Option<BlockTuple>, E2sError> {
        loop {
            let Some(entry) = self.reader.read_next_entry()? else {
                return Ok(None);
            };

            match entry.entry_type {
                execution_types::COMPRESSED_HEADER => {
                    self.headers.push_back(CompressedHeader::from_entry(&entry)?);
                }
                execution_types::COMPRESSED_BODY => {
                    self.bodies.push_back(CompressedBody::from_entry(&entry)?);
                }
                execution_types::COMPRESSED_RECEIPTS => {
                    self.receipts.push_back(CompressedReceipts::from_entry(&entry)?);
                }
                execution_types::TOTAL_DIFFICULTY => {
                    self.difficulties.push_back(TotalDifficulty::from_entry(&entry)?);
                }
                execution_types::ACCUMULATOR => {
                    if self.accumulator.is_some() {
                        return Err(E2sError::Ssz("Multiple accumulator entries found".to_string()));
                    }
                    self.accumulator = Some(Accumulator::from_entry(&entry)?);
                }
                BLOCK_INDEX => {
                    if self.block_index.is_some() {
                        return Err(E2sError::Ssz("Multiple block index entries found".to_string()));
                    }
                    self.block_index = Some(BlockIndex::from_entry(&entry)?);
                }
                _ => {
                    self.other_entries.push(entry);
                }
            }

            if !self.headers.is_empty() &&
                !self.bodies.is_empty() &&
                !self.receipts.is_empty() &&
                !self.difficulties.is_empty()
            {
                let header = self.headers.pop_front().unwrap();
                let body = self.bodies.pop_front().unwrap();
                let receipt = self.receipts.pop_front().unwrap();
                let difficulty = self.difficulties.pop_front().unwrap();

                return Ok(Some(BlockTuple::new(header, body, receipt, difficulty)));
            }
        }
    }
}

impl<R: Read + Seek> Era1Reader<R> {
    /// Create a new [`Era1Reader`]
    pub fn new(reader: R) -> Self {
        Self { reader: E2StoreReader::new(reader) }
    }

    /// Returns an iterator of [`BlockTuple`] streaming from `reader`.
    pub fn iter(&mut self) -> BlockTupleIterator<'_, R> {
        BlockTupleIterator::new(&mut self.reader)
    }

    /// Reads and parses an Era1 file from the underlying reader, assembling all components
    /// into a complete [`Era1File`] with an [`Era1Id`] that includes the provided network name.
    pub fn read(&mut self, network_name: String) -> Result<Era1File, E2sError> {
        // Validate version entry
        let _version_entry = match self.reader.read_version()? {
            Some(entry) if entry.is_version() => entry,
            Some(_) => return Err(E2sError::Ssz("First entry is not a Version entry".to_string())),
            None => return Err(E2sError::Ssz("Empty Era1 file".to_string())),
        };

        let mut iter = self.iter();
        let blocks = (&mut iter).collect::<Result<Vec<_>, _>>()?;

        let BlockTupleIterator {
            headers,
            bodies,
            receipts,
            difficulties,
            other_entries,
            accumulator,
            block_index,
            ..
        } = iter;

        // Ensure we have matching counts for block components
        if headers.len() != bodies.len() ||
            headers.len() != receipts.len() ||
            headers.len() != difficulties.len()
        {
            return Err(E2sError::Ssz(format!(
                "Mismatched block component counts: headers={}, bodies={}, receipts={}, difficulties={}",
                headers.len(), bodies.len(), receipts.len(), difficulties.len()
            )));
        }

        let accumulator = accumulator
            .ok_or_else(|| E2sError::Ssz("Era1 file missing accumulator entry".to_string()))?;

        let block_index = block_index
            .ok_or_else(|| E2sError::Ssz("Era1 file missing block index entry".to_string()))?;

        let mut group = Era1Group::new(blocks, accumulator, block_index.clone());

        // Add other entries
        for entry in other_entries {
            group.add_entry(entry);
        }

        let id = Era1Id::new(
            network_name,
            block_index.starting_number,
            block_index.offsets.len() as u32,
        );

        Ok(Era1File::new(group, id))
    }
}

impl Era1Reader<File> {
    /// Opens and reads an Era1 file from the given path
    pub fn open<P: AsRef<Path>>(
        path: P,
        network_name: impl Into<String>,
    ) -> Result<Era1File, E2sError> {
        let file = File::open(path).map_err(E2sError::Io)?;
        let mut reader = Self::new(file);
        reader.read(network_name.into())
    }
}

/// Writer for Era1 files that builds on top of [`E2StoreWriter`]
#[derive(Debug)]
pub struct Era1Writer<W: Write> {
    writer: E2StoreWriter<W>,
    has_written_version: bool,
    has_written_blocks: bool,
    has_written_accumulator: bool,
    has_written_block_index: bool,
}

impl<W: Write> Era1Writer<W> {
    /// Create a new [`Era1Writer`]
    pub fn new(writer: W) -> Self {
        Self {
            writer: E2StoreWriter::new(writer),
            has_written_version: false,
            has_written_blocks: false,
            has_written_accumulator: false,
            has_written_block_index: false,
        }
    }

    /// Write the version entry
    pub fn write_version(&mut self) -> Result<(), E2sError> {
        if self.has_written_version {
            return Ok(());
        }

        self.writer.write_version()?;
        self.has_written_version = true;
        Ok(())
    }

    /// Write a complete [`Era1File`] to the underlying writer
    pub fn write_era1_file(&mut self, era1_file: &Era1File) -> Result<(), E2sError> {
        // Write version
        self.write_version()?;

        // Ensure blocks are written before other entries
        if era1_file.group.blocks.len() > MAX_BLOCKS_PER_ERA1 {
            return Err(E2sError::Ssz("Era1 file cannot contain more than 8192 blocks".to_string()));
        }

        // Write all blocks
        for block in &era1_file.group.blocks {
            self.write_block(block)?;
        }

        // Write other entries
        for entry in &era1_file.group.other_entries {
            self.writer.write_entry(entry)?;
        }

        // Write accumulator
        self.write_accumulator(&era1_file.group.accumulator)?;

        // Write block index
        self.write_block_index(&era1_file.group.block_index)?;

        // Flush the writer
        self.writer.flush()?;

        Ok(())
    }

    /// Write a single block tuple
    pub fn write_block(
        &mut self,
        block_tuple: &crate::execution_types::BlockTuple,
    ) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        if self.has_written_accumulator || self.has_written_block_index {
            return Err(E2sError::Ssz(
                "Cannot write blocks after accumulator or block index".to_string(),
            ));
        }

        // Write header
        let header_entry = block_tuple.header.to_entry();
        self.writer.write_entry(&header_entry)?;

        // Write body
        let body_entry = block_tuple.body.to_entry();
        self.writer.write_entry(&body_entry)?;

        // Write receipts
        let receipts_entry = block_tuple.receipts.to_entry();
        self.writer.write_entry(&receipts_entry)?;

        // Write difficulty
        let difficulty_entry = block_tuple.total_difficulty.to_entry();
        self.writer.write_entry(&difficulty_entry)?;

        self.has_written_blocks = true;

        Ok(())
    }

    /// Write the accumulator
    pub fn write_accumulator(&mut self, accumulator: &Accumulator) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        if self.has_written_accumulator {
            return Err(E2sError::Ssz("Accumulator already written".to_string()));
        }

        if self.has_written_block_index {
            return Err(E2sError::Ssz("Cannot write accumulator after block index".to_string()));
        }

        let accumulator_entry = accumulator.to_entry();
        self.writer.write_entry(&accumulator_entry)?;
        self.has_written_accumulator = true;

        Ok(())
    }

    /// Write the block index
    pub fn write_block_index(&mut self, block_index: &BlockIndex) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        if self.has_written_block_index {
            return Err(E2sError::Ssz("Block index already written".to_string()));
        }

        let block_index_entry = block_index.to_entry();
        self.writer.write_entry(&block_index_entry)?;
        self.has_written_block_index = true;

        Ok(())
    }

    /// Flush any buffered data to the underlying writer
    pub fn flush(&mut self) -> Result<(), E2sError> {
        self.writer.flush()
    }
}

impl Era1Writer<File> {
    /// Creates a new file at the specified path and writes the [`Era1File`] to it
    pub fn create<P: AsRef<Path>>(path: P, era1_file: &Era1File) -> Result<(), E2sError> {
        let file = File::create(path).map_err(E2sError::Io)?;
        let mut writer = Self::new(file);
        writer.write_era1_file(era1_file)?;
        Ok(())
    }

    /// Creates a new file in the specified directory with a filename derived from the
    /// [`Era1File`]'s ID using the standardized Era1 file naming convention
    pub fn create_with_id<P: AsRef<Path>>(
        directory: P,
        era1_file: &Era1File,
    ) -> Result<(), E2sError> {
        let filename = era1_file.id.to_file_name();
        let path = directory.as_ref().join(filename);
        Self::create(path, era1_file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_types::{
        Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts,
        TotalDifficulty,
    };
    use alloy_primitives::{B256, U256};
    use std::io::Cursor;
    use tempfile::tempdir;

    // Helper to create a sample block tuple for testing
    fn create_test_block(number: BlockNumber, data_size: usize) -> BlockTuple {
        let header_data = vec![(number % 256) as u8; data_size];
        let header = CompressedHeader::new(header_data);

        let body_data = vec![((number + 1) % 256) as u8; data_size * 2];
        let body = CompressedBody::new(body_data);

        let receipts_data = vec![((number + 2) % 256) as u8; data_size];
        let receipts = CompressedReceipts::new(receipts_data);

        let difficulty = TotalDifficulty::new(U256::from(number * 1000));

        BlockTuple::new(header, body, receipts, difficulty)
    }

    // Helper to create a sample Era1File for testing
    fn create_test_era1_file(
        start_block: BlockNumber,
        block_count: usize,
        network: &str,
    ) -> Era1File {
        // Create blocks
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let block_num = start_block + i as u64;
            blocks.push(create_test_block(block_num, 32));
        }

        let accumulator = Accumulator::new(B256::from([0xAA; 32]));

        let mut offsets = Vec::with_capacity(block_count);
        for i in 0..block_count {
            offsets.push(i as i64 * 100);
        }
        let block_index = BlockIndex::new(start_block, offsets);
        let group = Era1Group::new(blocks, accumulator, block_index);
        let id = Era1Id::new(network, start_block, block_count as u32);

        Era1File::new(group, id)
    }

    #[test]
    fn test_era1_roundtrip_memory() -> Result<(), E2sError> {
        // Create a test Era1File
        let start_block = 1000;
        let era1_file = create_test_era1_file(1000, 5, "testnet");

        // Write to memory buffer
        let mut buffer = Vec::new();
        {
            let mut writer = Era1Writer::new(&mut buffer);
            writer.write_era1_file(&era1_file)?;
        }

        // Read back from memory buffer
        let mut reader = Era1Reader::new(Cursor::new(&buffer));
        let read_era1 = reader.read("testnet".to_string())?;

        // Verify core properties
        assert_eq!(read_era1.id.network_name, "testnet");
        assert_eq!(read_era1.id.start_block, 1000);
        assert_eq!(read_era1.id.block_count, 5);
        assert_eq!(read_era1.group.blocks.len(), 5);

        // Verify block properties
        assert_eq!(read_era1.group.blocks[0].total_difficulty.value, U256::from(1000 * 1000));
        assert_eq!(read_era1.group.blocks[1].total_difficulty.value, U256::from(1001 * 1000));

        // Verify block data
        assert_eq!(read_era1.group.blocks[0].header.data, vec![(start_block % 256) as u8; 32]);
        assert_eq!(read_era1.group.blocks[0].body.data, vec![((start_block + 1) % 256) as u8; 64]);
        assert_eq!(
            read_era1.group.blocks[0].receipts.data,
            vec![((start_block + 2) % 256) as u8; 32]
        );

        // Verify block access methods
        assert!(read_era1.contains_block(1000));
        assert!(read_era1.contains_block(1004));
        assert!(!read_era1.contains_block(999));
        assert!(!read_era1.contains_block(1005));

        let block_1002 = read_era1.get_block_by_number(1002);
        assert!(block_1002.is_some());
        assert_eq!(block_1002.unwrap().header.data, vec![((start_block + 2) % 256) as u8; 32]);

        Ok(())
    }

    #[test]
    fn test_era1_roundtrip_file() -> Result<(), E2sError> {
        // Create a temporary directory
        let temp_dir = tempdir().expect("Failed to create temp directory");
        let file_path = temp_dir.path().join("test_roundtrip.era1");

        // Create and write `Era1File` to disk
        let era1_file = create_test_era1_file(2000, 3, "mainnet");
        Era1Writer::create(&file_path, &era1_file)?;

        // Read it back
        let read_era1 = Era1Reader::open(&file_path, "mainnet")?;

        // Verify core properties
        assert_eq!(read_era1.id.network_name, "mainnet");
        assert_eq!(read_era1.id.start_block, 2000);
        assert_eq!(read_era1.id.block_count, 3);
        assert_eq!(read_era1.group.blocks.len(), 3);

        // Verify blocks
        for i in 0..3 {
            let block_num = 2000 + i as u64;
            let block = read_era1.get_block_by_number(block_num);
            assert!(block.is_some());
            assert_eq!(block.unwrap().header.data, vec![block_num as u8; 32]);
        }

        Ok(())
    }
}
