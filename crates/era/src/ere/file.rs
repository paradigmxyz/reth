//! Represents a complete `ere` (era execution) file.
//!
//! The structure of an `ere` file follows the specification:
//! `Version | CompressedHeader+ | CompressedBody+ | CompressedSlimReceipts* | Proof* |
//! TotalDifficulty* | other-entries* | Accumulator? | DynamicBlockIndex`
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md>.

use crate::{
    common::file_ops::{EraFileFormat, StreamReader, StreamWriter},
    e2s::{
        error::E2sError,
        file::{E2StoreReader, E2StoreWriter},
        types::{Entry, Version},
    },
    ere::types::{
        execution::{
            Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedSlimReceipts,
            Proof, TotalDifficulty, ACCUMULATOR, COMPRESSED_BODY, COMPRESSED_HEADER,
            COMPRESSED_SLIM_RECEIPTS, MAX_BLOCKS_PER_ERE, PROOF, TOTAL_DIFFICULTY,
        },
        group::{DynamicBlockIndex, EreGroup, EreId, DYNAMIC_BLOCK_INDEX},
    },
};
use std::{
    collections::VecDeque,
    io::{Read, Seek, Write},
};

/// `ere` file interface
#[derive(Debug)]
pub struct EreFile {
    /// Version record, must be the first record in the file
    pub version: Version,

    /// Main content group of the `ere` file
    pub group: EreGroup,

    /// File identifier
    pub id: EreId,
}

impl EraFileFormat for EreFile {
    type EraGroup = EreGroup;
    type Id = EreId;

    /// Create a new [`EreFile`]
    fn new(group: EreGroup, id: EreId) -> Self {
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

/// Reader for `ere` files that builds on top of [`E2StoreReader`]
#[derive(Debug)]
pub struct EreReader<R: Read> {
    reader: E2StoreReader<R>,
}

/// An iterator over the [`BlockTuple`]s in an `ere` file.
///
/// Records are grouped by type rather than per block, so a complete tuple spans the entire stream.
/// Iteration is therefore eager: the full set of blocks is materialized up front and yielded from
/// that buffer, rather than decoded incrementally per item.
#[derive(Debug)]
pub struct EreBlockTupleIterator<R: Read> {
    reader: E2StoreReader<R>,
    blocks: VecDeque<BlockTuple>,
    accumulator: Option<Accumulator>,
    index: Option<DynamicBlockIndex>,
    other_entries: Vec<Entry>,
    loaded: bool,
}

impl<R: Read> EreBlockTupleIterator<R> {
    const fn new(reader: E2StoreReader<R>) -> Self {
        Self {
            reader,
            blocks: VecDeque::new(),
            accumulator: None,
            index: None,
            other_entries: Vec::new(),
            loaded: false,
        }
    }
}

impl<R: Read + Seek> EreBlockTupleIterator<R> {
    /// Drain every remaining entry, bucketing it by record type, then assemble the block tuples.
    fn load(&mut self) -> Result<(), E2sError> {
        if self.loaded {
            return Ok(());
        }
        // Set before reading so a mid-stream error is surfaced exactly once.
        self.loaded = true;

        let mut headers = Vec::new();
        let mut bodies = Vec::new();
        let mut receipts = Vec::new();
        let mut proofs = Vec::new();
        let mut difficulties = Vec::new();

        while let Some(entry) = self.reader.read_next_entry()? {
            match entry.entry_type {
                COMPRESSED_HEADER => headers.push(CompressedHeader::from_entry(&entry)?),
                COMPRESSED_BODY => bodies.push(CompressedBody::from_entry(&entry)?),
                COMPRESSED_SLIM_RECEIPTS => {
                    receipts.push(CompressedSlimReceipts::from_entry(&entry)?)
                }
                PROOF => proofs.push(Proof::from_entry(&entry)?),
                TOTAL_DIFFICULTY => difficulties.push(TotalDifficulty::from_entry(&entry)?),
                ACCUMULATOR => {
                    if self.accumulator.is_some() {
                        return Err(E2sError::Ssz("Multiple accumulator entries found".to_string()));
                    }
                    self.accumulator = Some(Accumulator::from_entry(&entry)?);
                }
                DYNAMIC_BLOCK_INDEX => {
                    if self.index.is_some() {
                        return Err(E2sError::Ssz("Multiple block index entries found".to_string()));
                    }
                    self.index = Some(DynamicBlockIndex::from_entry(&entry)?);
                }
                _ => self.other_entries.push(entry),
            }
        }

        self.blocks = assemble_blocks(headers, bodies, receipts, proofs, difficulties)?;
        Ok(())
    }
}

impl<R: Read + Seek> Iterator for EreBlockTupleIterator<R> {
    type Item = Result<BlockTuple, E2sError>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.loaded &&
            let Err(err) = self.load()
        {
            return Some(Err(err));
        }
        self.blocks.pop_front().map(Ok)
    }
}

impl<R: Read + Seek> StreamReader<R> for EreReader<R> {
    type File = EreFile;
    type Iterator = EreBlockTupleIterator<R>;

    /// Create a new [`EreReader`]
    fn new(reader: R) -> Self {
        Self { reader: E2StoreReader::new(reader) }
    }

    /// Returns an iterator over the [`BlockTuple`]s streamed from `reader`.
    fn iter(self) -> EreBlockTupleIterator<R> {
        EreBlockTupleIterator::new(self.reader)
    }

    fn read(self, network_name: String) -> Result<Self::File, E2sError> {
        self.read_and_assemble(network_name)
    }
}

impl<R: Read + Seek> EreReader<R> {
    /// Reads and parses an `ere` file from the underlying reader, assembling all components into a
    /// complete [`EreFile`] with an [`EreId`] that includes the provided network name.
    pub fn read_and_assemble(mut self, network_name: String) -> Result<EreFile, E2sError> {
        // Validate the version entry before draining the rest of the stream.
        match self.reader.read_version()? {
            Some(entry) if entry.is_version() => {}
            Some(_) => return Err(E2sError::Ssz("First entry is not a Version entry".to_string())),
            None => return Err(E2sError::Ssz("Empty ere file".to_string())),
        }

        let mut iter = self.iter();
        let blocks = (&mut iter).collect::<Result<Vec<_>, _>>()?;

        let EreBlockTupleIterator { accumulator, index, other_entries, .. } = iter;

        let index =
            index.ok_or_else(|| E2sError::Ssz("ere file missing block index entry".to_string()))?;

        let mut group = EreGroup::new(blocks, accumulator, index.clone());
        for entry in other_entries {
            group.add_entry(entry);
        }

        let id = EreId::new(network_name, index.starting_number(), index.block_count() as u32);

        Ok(EreFile::new(group, id))
    }
}

/// Writer for `ere` files that builds on top of [`E2StoreWriter`]
#[derive(Debug)]
pub struct EreWriter<W: Write> {
    writer: E2StoreWriter<W>,
}

impl<W: Write> StreamWriter<W> for EreWriter<W> {
    type File = EreFile;

    /// Create a new [`EreWriter`]
    fn new(writer: W) -> Self {
        Self { writer: E2StoreWriter::new(writer) }
    }

    /// Write the version entry
    fn write_version(&mut self) -> Result<(), E2sError> {
        self.writer.write_version()
    }

    /// Write a complete [`EreFile`] to the underlying writer.
    ///
    /// Records are emitted in the spec's sectioned order: all headers, all bodies, then each
    /// optional section (receipts, proofs, total-difficulty), followed by any other entries, the
    /// optional accumulator, and finally the mandatory block index.
    fn write_file(&mut self, file: &EreFile) -> Result<(), E2sError> {
        self.write_version()?;

        let blocks = &file.group.blocks;
        if blocks.len() > MAX_BLOCKS_PER_ERE {
            return Err(E2sError::Ssz(format!(
                "ere file cannot contain more than {MAX_BLOCKS_PER_ERE} blocks"
            )));
        }

        for block in blocks {
            self.writer.write_entry(&block.header.to_entry())?;
        }
        for block in blocks {
            self.writer.write_entry(&block.body.to_entry())?;
        }
        for block in blocks {
            if let Some(receipts) = &block.receipts {
                self.writer.write_entry(&receipts.to_entry())?;
            }
        }
        for block in blocks {
            if let Some(proof) = &block.proof {
                self.writer.write_entry(&proof.to_entry())?;
            }
        }
        for block in blocks {
            if let Some(total_difficulty) = &block.total_difficulty {
                self.writer.write_entry(&total_difficulty.to_entry())?;
            }
        }

        for entry in &file.group.other_entries {
            self.writer.write_entry(entry)?;
        }

        if let Some(accumulator) = &file.group.accumulator {
            self.writer.write_entry(&accumulator.to_entry())?;
        }

        self.writer.write_entry(&file.group.index.to_entry())?;

        self.writer.flush()?;
        Ok(())
    }

    /// Flush any buffered data to the underlying writer
    fn flush(&mut self) -> Result<(), E2sError> {
        self.writer.flush()
    }
}

/// Zip the per-type sections of an `ere` file back into [`BlockTuple`]s.
///
/// Headers and bodies are mandatory and must be equinumerous. Each optional section must be either
/// empty (absent for the whole file) or carry exactly one entry per block, mirroring the single,
/// uniform `component-count` the spec stores in the [`DynamicBlockIndex`].
fn assemble_blocks(
    headers: Vec<CompressedHeader>,
    bodies: Vec<CompressedBody>,
    receipts: Vec<CompressedSlimReceipts>,
    proofs: Vec<Proof>,
    difficulties: Vec<TotalDifficulty>,
) -> Result<VecDeque<BlockTuple>, E2sError> {
    let block_count = headers.len();
    if bodies.len() != block_count {
        return Err(E2sError::Ssz(format!(
            "Mismatched header/body counts: headers={block_count}, bodies={}",
            bodies.len()
        )));
    }

    let mut receipts = optional_section(receipts, block_count)?;
    let mut difficulties = optional_section(difficulties, block_count)?;
    let mut proofs = optional_section(proofs, block_count)?;

    let mut blocks = VecDeque::with_capacity(block_count);
    for (header, body) in headers.into_iter().zip(bodies) {
        let mut block = BlockTuple::new(header, body);
        if let Some(receipts) = receipts.next().flatten() {
            block = block.with_receipts(receipts);
        }
        if let Some(difficulty) = difficulties.next().flatten() {
            block = block.with_total_difficulty(difficulty);
        }
        if let Some(proof) = proofs.next().flatten() {
            block = block.with_proof(proof);
        }
        blocks.push_back(block);
    }

    Ok(blocks)
}

/// Expand an optional component column into one `Option<T>` per block.
///
/// The `component-count` is file-uniform, so a component is encoded for every block or for none: an
/// empty column maps to `block_count` `None`s and a full column to its wrapped values. Any other
/// length breaks the column/block alignment and is rejected.
fn optional_section<T>(
    section: Vec<T>,
    block_count: usize,
) -> Result<impl Iterator<Item = Option<T>>, E2sError> {
    let slots: Vec<Option<T>> = match section.len() {
        0 => (0..block_count).map(|_| None).collect(),
        n if n == block_count => section.into_iter().map(Some).collect(),
        n => {
            return Err(E2sError::Ssz(format!(
                "ere optional section length must be 0 or {block_count}, got {n}"
            )))
        }
    };
    Ok(slots.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, U256};
    use std::io::Cursor;

    /// Build a test block tuple. Optional components are attached based on the flags so tests can
    /// exercise the full 2-5 component-count range.
    fn create_test_block(
        number: u64,
        data_size: usize,
        with_receipts: bool,
        with_difficulty: bool,
        with_proof: bool,
    ) -> BlockTuple {
        let header = CompressedHeader::new(vec![(number % 256) as u8; data_size]);
        let body = CompressedBody::new(vec![((number + 1) % 256) as u8; data_size * 2]);

        let mut block = BlockTuple::new(header, body);
        if with_receipts {
            block = block.with_receipts(CompressedSlimReceipts::new(vec![
                ((number + 2) % 256)
                    as u8;
                data_size
            ]));
        }
        if with_difficulty {
            block = block.with_total_difficulty(TotalDifficulty::new(U256::from(number * 1000)));
        }
        if with_proof {
            block = block.with_proof(Proof::new(vec![((number + 3) % 256) as u8; data_size]));
        }
        block
    }

    /// Build a test [`EreFile`]. The block index records the component-count derived from the first
    /// block, mirroring the uniform layout the spec mandates.
    fn create_test_ere_file(
        start_block: u64,
        block_count: usize,
        network: &str,
        with_receipts: bool,
        with_difficulty: bool,
        with_proof: bool,
        with_accumulator: bool,
    ) -> EreFile {
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            blocks.push(create_test_block(
                start_block + i as u64,
                32,
                with_receipts,
                with_difficulty,
                with_proof,
            ));
        }

        let component_count = blocks.first().map_or(2, BlockTuple::component_count);
        // Offsets are placeholders here; the index is stored and read back verbatim, so the values
        // are not interpreted when assembling blocks.
        let offsets = (0..(block_count as u64 * component_count) as i64).collect();
        let index = DynamicBlockIndex::new(start_block, component_count, offsets);

        let accumulator = with_accumulator.then(|| Accumulator::new(B256::from([0xAA; 32])));
        let group = EreGroup::new(blocks, accumulator, index);
        let id = EreId::new(network, start_block, block_count as u32);

        EreFile::new(group, id)
    }

    /// Assert two block tuples carry identical compressed payloads and optional components.
    fn assert_block_eq(actual: &BlockTuple, expected: &BlockTuple) {
        assert_eq!(actual.header.data, expected.header.data);
        assert_eq!(actual.body.data, expected.body.data);
        assert_eq!(
            actual.receipts.as_ref().map(|r| &r.data),
            expected.receipts.as_ref().map(|r| &r.data)
        );
        assert_eq!(
            actual.total_difficulty.as_ref().map(|d| d.value),
            expected.total_difficulty.as_ref().map(|d| d.value)
        );
        assert_eq!(
            actual.proof.as_ref().map(|p| &p.data),
            expected.proof.as_ref().map(|p| &p.data)
        );
    }

    /// Write `original` to an in-memory buffer and read it back.
    fn write_then_read(original: &EreFile, network: &str) -> EreFile {
        let mut buffer = Vec::new();
        EreWriter::new(&mut buffer).write_file(original).expect("write");
        EreReader::new(Cursor::new(&buffer)).read(network.to_string()).expect("read")
    }

    #[test]
    fn test_ere_write_read_all_components() {
        // Pre-merge file: every optional component present (component-count 5) plus accumulator.
        let original = create_test_ere_file(1000, 4, "testnet", true, true, true, true);
        let read = write_then_read(&original, "testnet");

        assert_eq!(read.id.network_name, "testnet");
        assert_eq!(read.id.start_block, 1000);
        assert_eq!(read.id.block_count, 4);
        assert_eq!(read.group.blocks.len(), 4);
        assert_eq!(read.group.index, original.group.index);
        assert_eq!(read.group.accumulator, original.group.accumulator);

        for (read_block, original_block) in read.group.blocks.iter().zip(&original.group.blocks) {
            assert_eq!(read_block.component_count(), 5);
            assert_block_eq(read_block, original_block);
        }
    }

    #[test]
    fn test_ere_write_read_header_body_only() {
        // Post-merge subset file: only header + body (component-count 2), no accumulator.
        let original = create_test_ere_file(2000, 3, "mainnet", false, false, false, false);
        let read = write_then_read(&original, "mainnet");

        assert_eq!(read.group.blocks.len(), 3);
        assert!(read.group.accumulator.is_none());
        assert_eq!(read.group.index, original.group.index);

        for (read_block, original_block) in read.group.blocks.iter().zip(&original.group.blocks) {
            assert_eq!(read_block.component_count(), 2);
            assert!(read_block.receipts.is_none());
            assert!(read_block.total_difficulty.is_none());
            assert!(read_block.proof.is_none());
            assert_block_eq(read_block, original_block);
        }
    }

    #[test]
    fn test_ere_write_read_partial_components() {
        // `noproofs` profile: receipts and difficulty present, proofs absent (component-count 4).
        let original = create_test_ere_file(0, 2, "sepolia", true, true, false, true);
        let read = write_then_read(&original, "sepolia");

        for block in &read.group.blocks {
            assert_eq!(block.component_count(), 4);
            assert!(block.receipts.is_some());
            assert!(block.total_difficulty.is_some());
            assert!(block.proof.is_none());
        }
    }

    #[test]
    fn test_ere_write_read_preserves_other_entries() {
        let mut original = create_test_ere_file(1000, 1, "testnet", true, true, true, true);
        original.group.add_entry(Entry::new([0x42, 0x42], vec![1, 2, 3, 4]));

        let read = write_then_read(&original, "testnet");

        assert_eq!(read.group.other_entries.len(), 1);
        assert_eq!(read.group.other_entries[0].entry_type, [0x42, 0x42]);
        assert_eq!(read.group.other_entries[0].data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_ere_read_rejects_missing_index() {
        // A file with blocks but no DynamicBlockIndex must be rejected.
        let mut buffer = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer);
            writer.write_version().unwrap();
            let block = create_test_block(1, 8, false, false, false);
            writer.write_entry(&block.header.to_entry()).unwrap();
            writer.write_entry(&block.body.to_entry()).unwrap();
            writer.flush().unwrap();
        }

        let err = EreReader::new(Cursor::new(&buffer)).read("testnet".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn test_ere_read_rejects_partial_optional_section() {
        // Two blocks but only one receipts entry: the optional section is neither empty nor 1-per
        // block, so assembly must fail.
        let mut buffer = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer);
            writer.write_version().unwrap();
            let b0 = create_test_block(0, 8, false, false, false);
            let b1 = create_test_block(1, 8, false, false, false);
            writer.write_entry(&b0.header.to_entry()).unwrap();
            writer.write_entry(&b1.header.to_entry()).unwrap();
            writer.write_entry(&b0.body.to_entry()).unwrap();
            writer.write_entry(&b1.body.to_entry()).unwrap();
            // Only one receipts entry for two blocks.
            writer.write_entry(&CompressedSlimReceipts::new(vec![0xCC; 8]).to_entry()).unwrap();
            writer.write_entry(&DynamicBlockIndex::new(0, 2, vec![0, 1, 2, 3]).to_entry()).unwrap();
            writer.flush().unwrap();
        }

        let err = EreReader::new(Cursor::new(&buffer)).read("testnet".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn test_ere_write_read_empty() {
        // No blocks: the header/body sections are empty and the index covers zero blocks.
        let original = create_test_ere_file(0, 0, "testnet", false, false, false, false);
        let read = write_then_read(&original, "testnet");

        assert_eq!(read.group.blocks.len(), 0);
        assert_eq!(read.id.block_count, 0);
        assert_eq!(read.group.index, original.group.index);
    }

    #[test]
    fn test_ere_read_rejects_empty_input() {
        // No version record at all.
        let err = EreReader::new(Cursor::new(Vec::new())).read("testnet".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn test_ere_read_rejects_non_version_first_entry() {
        // The first record must be the version; a stray entry up front is rejected.
        let mut buffer = Vec::new();
        Entry::new([0x99, 0x99], vec![1, 2, 3]).write(&mut buffer).unwrap();

        let err = EreReader::new(Cursor::new(&buffer)).read("testnet".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn test_ere_read_rejects_header_body_mismatch() {
        // Two headers but one body: the mandatory sections must be equinumerous.
        let mut buffer = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer);
            writer.write_version().unwrap();
            let b0 = create_test_block(0, 8, false, false, false);
            let b1 = create_test_block(1, 8, false, false, false);
            writer.write_entry(&b0.header.to_entry()).unwrap();
            writer.write_entry(&b1.header.to_entry()).unwrap();
            writer.write_entry(&b0.body.to_entry()).unwrap();
            writer.write_entry(&DynamicBlockIndex::new(0, 2, vec![0, 1, 2, 3]).to_entry()).unwrap();
            writer.flush().unwrap();
        }

        let err = EreReader::new(Cursor::new(&buffer)).read("testnet".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn test_ere_read_rejects_duplicate_index() {
        // A second DynamicBlockIndex record must be rejected.
        let mut buffer = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer);
            writer.write_version().unwrap();
            writer.write_entry(&DynamicBlockIndex::new(0, 2, Vec::new()).to_entry()).unwrap();
            writer.write_entry(&DynamicBlockIndex::new(0, 2, Vec::new()).to_entry()).unwrap();
            writer.flush().unwrap();
        }

        let err = EreReader::new(Cursor::new(&buffer)).read("testnet".to_string());
        assert!(err.is_err());
    }

    #[test]
    fn test_ere_read_rejects_duplicate_accumulator() {
        // A second Accumulator record must be rejected.
        let mut buffer = Vec::new();
        {
            let mut writer = E2StoreWriter::new(&mut buffer);
            writer.write_version().unwrap();
            writer.write_entry(&Accumulator::new(B256::from([0x11; 32])).to_entry()).unwrap();
            writer.write_entry(&Accumulator::new(B256::from([0x22; 32])).to_entry()).unwrap();
            writer.write_entry(&DynamicBlockIndex::new(0, 2, Vec::new()).to_entry()).unwrap();
            writer.flush().unwrap();
        }

        let err = EreReader::new(Cursor::new(&buffer)).read("testnet".to_string());
        assert!(err.is_err());
    }
}
