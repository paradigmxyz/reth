//! Represents a complete Era1 file
//!
//! The structure of an Era1 file follows the specification:
//! `Version | block-tuple* | other-entries* | Accumulator | BlockIndex`
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>.

use crate::{
    common::file_ops::{EraFileFormat, StreamReader, StreamWriter},
    e2s::{
        error::E2sError,
        file::{E2StoreReader, E2StoreWriter},
        types::{Entry, IndexEntry, Version},
    },
    era1::types::{
        execution::{
            Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts,
            TotalDifficulty, ACCUMULATOR, COMPRESSED_BODY, COMPRESSED_HEADER, COMPRESSED_RECEIPTS,
            MAX_BLOCKS_PER_ERA1, TOTAL_DIFFICULTY,
        },
        group::{BlockIndex, Era1Group, Era1Id, BLOCK_INDEX},
    },
};
use alloy_primitives::BlockNumber;
use std::{
    collections::VecDeque,
    io::{Read, Seek, Write},
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

impl EraFileFormat for Era1File {
    type EraGroup = Era1Group;
    type Id = Era1Id;

    /// Create a new [`Era1File`]
    fn new(group: Era1Group, id: Era1Id) -> Self {
        Self { version: Version, group, id }
    }

    fn version(&self) -> &Version {
        &self.version
    }

    fn group(&self) -> &Self::EraGroup {
        &self.group
    }

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl Era1File {
    /// Get a block by its number, if present in this file
    pub fn get_block_by_number(&self, number: BlockNumber) -> Option<&BlockTuple> {
        let index = (number - self.group.block_index.starting_number()) as usize;
        (index < self.group.blocks.len()).then(|| &self.group.blocks[index])
    }

    /// Get the range of block numbers contained in this file
    pub fn block_range(&self) -> std::ops::RangeInclusive<BlockNumber> {
        let start = self.group.block_index.starting_number();
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
///
/// Starts at the beginning of the file, validates the version before yielding blocks,
/// and checks for unmatched tuple
/// components and required accumulator/index records at EOF. Completion errors
/// are yielded as a final error, after which iteration ends. Stopping early skips
/// the completion checks. This does not recompute the accumulator or verify index offsets.
#[derive(Debug)]
pub struct BlockTupleIterator<R: Read> {
    reader: E2StoreReader<R>,
    headers: VecDeque<CompressedHeader>,
    bodies: VecDeque<CompressedBody>,
    receipts: VecDeque<CompressedReceipts>,
    difficulties: VecDeque<TotalDifficulty>,
    other_entries: Vec<Entry>,
    accumulator: Option<Accumulator>,
    block_index: Option<BlockIndex>,
    state: BlockTupleState,
}

impl<R: Read> BlockTupleIterator<R> {
    fn new(reader: E2StoreReader<R>) -> Self {
        Self {
            reader,
            headers: Default::default(),
            bodies: Default::default(),
            receipts: Default::default(),
            difficulties: Default::default(),
            other_entries: Default::default(),
            accumulator: None,
            block_index: None,
            state: BlockTupleState::Start,
        }
    }
}

impl<R: Read + Seek> Iterator for BlockTupleIterator<R> {
    type Item = Result<BlockTuple, E2sError>;

    fn next(&mut self) -> Option<Self::Item> {
        let result = self.next_result();
        if !matches!(result, Ok(Some(_))) {
            self.state = BlockTupleState::Finished;
        }
        result.transpose()
    }
}

impl<R: Read + Seek> std::iter::FusedIterator for BlockTupleIterator<R> {}

impl<R: Read + Seek> BlockTupleIterator<R> {
    fn next_result(&mut self) -> Result<Option<BlockTuple>, E2sError> {
        match self.state {
            BlockTupleState::Start => {
                self.state = BlockTupleState::Reading;
                if self.reader.read_version()?.is_none() {
                    return Err(E2sError::Ssz("Empty Era1 file".to_string()));
                }
            }
            BlockTupleState::Reading => {}
            BlockTupleState::Finished => return Ok(None),
        }

        loop {
            let Some(entry) = self.reader.read_next_entry()? else {
                self.validate_end()?;
                return Ok(None);
            };

            match entry.entry_type {
                COMPRESSED_HEADER => {
                    self.headers.push_back(CompressedHeader::from_entry(&entry)?);
                }
                COMPRESSED_BODY => {
                    self.bodies.push_back(CompressedBody::from_entry(&entry)?);
                }
                COMPRESSED_RECEIPTS => {
                    self.receipts.push_back(CompressedReceipts::from_entry(&entry)?);
                }
                TOTAL_DIFFICULTY => {
                    self.difficulties.push_back(TotalDifficulty::from_entry(&entry)?);
                }
                ACCUMULATOR => {
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

    fn validate_end(&self) -> Result<(), E2sError> {
        if self.headers.len() != self.bodies.len() ||
            self.headers.len() != self.receipts.len() ||
            self.headers.len() != self.difficulties.len()
        {
            return Err(E2sError::Ssz(format!(
                "Mismatched block component counts: headers={}, bodies={}, receipts={}, difficulties={}",
                self.headers.len(), self.bodies.len(), self.receipts.len(), self.difficulties.len()
            )));
        }

        if self.accumulator.is_none() {
            return Err(E2sError::Ssz("Era1 file missing accumulator entry".to_string()));
        }
        if self.block_index.is_none() {
            return Err(E2sError::Ssz("Era1 file missing block index entry".to_string()));
        }
        Ok(())
    }
}

impl<R: Read + Seek> StreamReader<R> for Era1Reader<R> {
    type File = Era1File;
    type Iterator = BlockTupleIterator<R>;

    /// Create a new [`Era1Reader`]
    fn new(reader: R) -> Self {
        Self { reader: E2StoreReader::new(reader) }
    }

    /// Returns a streaming iterator from the beginning of the file that validates the version
    /// and, when fully consumed, checks tuple completeness and required trailer records.
    /// See [`BlockTupleIterator`].
    fn iter(self) -> BlockTupleIterator<R> {
        BlockTupleIterator::new(self.reader)
    }

    fn read(self, network_name: String) -> Result<Self::File, E2sError> {
        self.read_and_assemble(network_name)
    }
}

impl<R: Read + Seek> Era1Reader<R> {
    /// Reads and parses an Era1 file from the underlying reader, assembling all components
    /// into a complete [`Era1File`] with an [`Era1Id`] that includes the provided network name.
    pub fn read_and_assemble(self, network_name: String) -> Result<Era1File, E2sError> {
        let mut iter = self.iter();
        let blocks = (&mut iter).collect::<Result<Vec<_>, _>>()?;

        let BlockTupleIterator { other_entries, accumulator, block_index, .. } = iter;

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
            block_index.starting_number(),
            block_index.offsets().len() as u32,
        );

        Ok(Era1File::new(group, id))
    }
}

/// Progress through the version, records, and terminal result of an Era1 stream.
#[derive(Debug)]
enum BlockTupleState {
    Start,
    Reading,
    Finished,
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

impl<W: Write> StreamWriter<W> for Era1Writer<W> {
    type File = Era1File;

    /// Create a new [`Era1Writer`]
    fn new(writer: W) -> Self {
        Self {
            writer: E2StoreWriter::new(writer),
            has_written_version: false,
            has_written_blocks: false,
            has_written_accumulator: false,
            has_written_block_index: false,
        }
    }

    /// Write the version entry
    fn write_version(&mut self) -> Result<(), E2sError> {
        if self.has_written_version {
            return Ok(());
        }

        self.writer.write_version()?;
        self.has_written_version = true;
        Ok(())
    }

    /// Write a complete [`Era1File`] to the underlying writer
    fn write_file(&mut self, era1_file: &Era1File) -> Result<(), E2sError> {
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

    /// Flush any buffered data to the underlying writer
    fn flush(&mut self) -> Result<(), E2sError> {
        self.writer.flush()
    }
}

impl<W: Write> Era1Writer<W> {
    /// Write a single block tuple
    pub fn write_block(&mut self, block_tuple: &BlockTuple) -> Result<(), E2sError> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        common::file_ops::{FileReader, FileWriter},
        e2s::types::{Header, VERSION},
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

        let accumulator = Accumulator::new(B256::repeat_byte(0xAA));

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
            writer.write_file(&era1_file)?;
        }

        // Read back from memory buffer
        let reader = Era1Reader::new(Cursor::new(&buffer));
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

    #[test]
    fn streaming_and_whole_file_reject_invalid_completion() {
        let block = create_test_block(1000, 32);
        let version = Entry::new(VERSION, Vec::new());
        let accumulator = Accumulator::new(B256::ZERO).to_entry();
        let index = BlockIndex::new(1000, vec![0]).to_entry();
        let block_entries = [
            block.header.to_entry(),
            block.body.to_entry(),
            block.receipts.to_entry(),
            block.total_difficulty.to_entry(),
        ];

        let cases = [
            (Vec::new(), "Empty Era1 file"),
            (vec![accumulator.clone(), index.clone()], "First entry must be a Version entry"),
            (vec![version.clone()], "Era1 file missing accumulator entry"),
            (vec![version.clone(), accumulator.clone()], "Era1 file missing block index entry"),
            (vec![version.clone(), index.clone()], "Era1 file missing accumulator entry"),
            (
                [vec![version.clone()], block_entries.to_vec()].concat(),
                "Era1 file missing accumulator entry",
            ),
            (
                [vec![version.clone()], block_entries.to_vec(), vec![accumulator.clone()]].concat(),
                "Era1 file missing block index entry",
            ),
            (
                vec![version.clone(), accumulator.clone(), accumulator.clone(), index.clone()],
                "Multiple accumulator entries found",
            ),
            (
                vec![version.clone(), accumulator.clone(), index.clone(), index.clone()],
                "Multiple block index entries found",
            ),
        ];

        for (entries, expected) in cases {
            assert_readers_reject(&entries, expected);
        }

        for (component, counts) in block_entries.iter().zip([
            "headers=1, bodies=0, receipts=0, difficulties=0",
            "headers=0, bodies=1, receipts=0, difficulties=0",
            "headers=0, bodies=0, receipts=1, difficulties=0",
            "headers=0, bodies=0, receipts=0, difficulties=1",
        ]) {
            let entries = [version.clone(), component.clone(), accumulator.clone(), index.clone()];
            assert_readers_reject(
                &entries,
                &format!("Mismatched block component counts: {counts}"),
            );
        }
    }

    #[test]
    fn streaming_validates_trailer_after_yielding_blocks() {
        let file = create_test_era1_file(1000, 1, "testnet");
        let mut buffer = Vec::new();
        {
            let mut writer = Era1Writer::new(&mut buffer);
            writer.write_block(&file.group.blocks[0]).unwrap();
            writer.flush().unwrap();
        }

        let mut iter = Era1Reader::new(Cursor::new(buffer)).iter();
        assert_eq!(iter.next().unwrap().unwrap().header.data, file.group.blocks[0].header.data);
        let E2sError::Ssz(err) = iter.next().unwrap().unwrap_err() else {
            panic!("expected a missing accumulator error");
        };
        assert_eq!(err, "Era1 file missing accumulator entry");
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[test]
    fn streaming_accepts_complete_files_and_preserves_other_entries() {
        for block_count in [0, 2] {
            let mut file = create_test_era1_file(1000, block_count, "testnet");
            file.group.add_entry(Entry::new([0x42, 0x42], vec![1, 2, 3]));
            let mut buffer = Vec::new();
            {
                let mut writer = Era1Writer::new(&mut buffer);
                writer.write_file(&file).unwrap();
            }

            let mut iter = Era1Reader::new(Cursor::new(&buffer)).iter();
            assert_eq!((&mut iter).collect::<Result<Vec<_>, _>>().unwrap().len(), block_count);
            assert!(iter.next().is_none());
            assert!(iter.next().is_none());

            let assembled =
                Era1Reader::new(Cursor::new(buffer)).read("testnet".to_string()).unwrap();
            assert_eq!(assembled.group.other_entries.len(), 1);
            assert_eq!(assembled.group.other_entries[0].entry_type, [0x42, 0x42]);
            assert_eq!(assembled.group.other_entries[0].data, [1, 2, 3]);
        }
    }

    #[test]
    fn streaming_io_error_terminates_iteration() {
        let mut data = Vec::new();
        Version.encode(&mut data).unwrap();
        Header::new(COMPRESSED_HEADER, 1).write(&mut data).unwrap();
        let mut iter = Era1Reader::new(Cursor::new(data)).iter();
        let E2sError::Io(err) = iter.next().unwrap().unwrap_err() else {
            panic!("expected a partial payload I/O error");
        };
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(iter.next().is_none());
    }

    fn assert_readers_reject(entries: &[Entry], expected: &str) {
        let mut data = Vec::new();
        for entry in entries {
            entry.write(&mut data).unwrap();
        }

        let mut iter = Era1Reader::new(Cursor::new(&data)).iter();
        let E2sError::Ssz(err) = (&mut iter).collect::<Result<Vec<_>, _>>().unwrap_err() else {
            panic!("expected an ERA1 validation error");
        };
        assert_eq!(err, expected);
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());

        let E2sError::Ssz(err) =
            Era1Reader::new(Cursor::new(data)).read("testnet".to_string()).unwrap_err()
        else {
            panic!("expected an ERA1 validation error");
        };
        assert_eq!(err, expected);
    }
}
