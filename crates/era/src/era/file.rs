//! Represents a complete Era file
//!
//! The structure of an Era file follows the specification:
//! `Version | block* | era-state | other-entries* | slot-index(block)? | slot-index(state)`
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>.

use crate::{
    common::file_ops::{EraFileFormat, FileReader, StreamReader, StreamWriter},
    e2s::{
        error::E2sError,
        file::{E2StoreReader, E2StoreWriter},
        types::{Entry, IndexEntry, Version, SLOT_INDEX},
    },
    era::types::{
        consensus::{
            CompressedBeaconState, CompressedSignedBeaconBlock, COMPRESSED_BEACON_STATE,
            COMPRESSED_SIGNED_BEACON_BLOCK,
        },
        group::{EraGroup, EraId, SlotIndex},
    },
    era1::types::execution::BlockTuple,
};

use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, Write},
};

/// Era file interface
#[derive(Debug)]
pub struct EraFile {
    /// Version record, must be the first record in the file
    pub version: Version,

    /// Main content group of the Era file
    pub group: EraGroup,

    /// File identifier
    pub id: EraId,
}

impl EraFileFormat for EraFile {
    type EraGroup = EraGroup;
    type Id = EraId;

    /// Create a new [`EraFile`]
    fn new(group: EraGroup, id: EraId) -> Self {
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

/// Reader for era files that builds on top of [`E2StoreReader`]
#[derive(Debug)]
pub struct EraReader<R: Read> {
    reader: E2StoreReader<R>,
}

/// An iterator of [`BeaconBlockIterator`] streaming from [`E2StoreReader`].
#[derive(Debug)]
#[allow(dead_code)]
pub struct BeaconBlockIterator<R: Read> {
    reader: E2StoreReader<R>,
    blocks: VecDeque<CompressedSignedBeaconBlock>,
    state: Option<CompressedBeaconState>,
    other_entries: Vec<Entry>,
    block_slot_index: Option<SlotIndex>,
    state_slot_index: Option<SlotIndex>,
}

impl<R: Read> BeaconBlockIterator<R> {
    fn new(reader: E2StoreReader<R>) -> Self {
        Self {
            reader,
            blocks: Default::default(),
            state: None,
            other_entries: Default::default(),
            block_slot_index: None,
            state_slot_index: None,
        }
    }
}

impl<R: Read + Seek> Iterator for BeaconBlockIterator<R> {
    type Item = Result<CompressedSignedBeaconBlock, E2sError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_result().transpose()
    }
}

impl<R: Read + Seek> BeaconBlockIterator<R> {
    fn next_result(&mut self) -> Result<Option<CompressedSignedBeaconBlock>, E2sError> {
        loop {
            let Some(entry) = self.reader.read_next_entry()? else {
                return Ok(None);
            };

            match entry.entry_type {
                COMPRESSED_SIGNED_BEACON_BLOCK => {
                    let block = CompressedSignedBeaconBlock::from_entry(&entry)?;
                    return Ok(Some(block));
                }
                COMPRESSED_BEACON_STATE => {
                    if self.state.is_some() {
                        return Err(E2sError::Ssz("Multiple state entries found".to_string()));
                    }
                    self.state = Some(CompressedBeaconState::from_entry(&entry)?);
                }
                SLOT_INDEX => {
                    let slot_index = SlotIndex::from_entry(&entry)?;
                    // Determine if this is block or state index based on what we've seen
                    if self.state.is_none() {
                        self.block_slot_index = Some(slot_index);
                    } else {
                        self.state_slot_index = Some(slot_index);
                    }
                }
                _ => {
                    self.other_entries.push(entry);
                }
            }
        }
    }
}

impl<R: Read + Seek> StreamReader<R> for EraReader<R> {
    type File = EraFile;
    type Iterator = BeaconBlockIterator<R>;

    /// Create a new [`EraReader`]
    fn new(reader: R) -> Self {
        Self { reader: E2StoreReader::new(reader) }
    }

    /// Returns an iterator of [`BeaconBlockIterator`] streaming from `reader`.
    fn iter(self) -> BeaconBlockIterator<R> {
        BeaconBlockIterator::new(self.reader)
    }

    fn read(self, network_name: String) -> Result<Self::File, E2sError> {
        self.read_and_assemble(network_name)
    }
}

impl<R: Read + Seek> EraReader<R> {
    /// Reads and parses an era file from the underlying reader, assembling all components
    /// into a complete [`EraFile`] with an [`EraId`] that includes the provided network name.
    pub fn read_and_assemble(mut self, network_name: String) -> Result<EraFile, E2sError> {
        // Validate version entry
        let _version_entry = match self.reader.read_version()? {
            Some(entry) if entry.is_version() => entry,
            Some(_) => return Err(E2sError::Ssz("First entry is not a Version entry".to_string())),
            None => return Err(E2sError::Ssz("Empty Era1 file".to_string())),
        };

        let mut iter = self.iter();
        let blocks = (&mut iter).collect::<Result<Vec<_>, _>>()?;

        let BeaconBlockIterator {
            state, other_entries, block_slot_index, state_slot_index, ..
        } = iter;

        let state =
            state.ok_or_else(|| E2sError::Ssz("Era file missing state entry".to_string()))?;

        let state_slot_index = state_slot_index
            .ok_or_else(|| E2sError::Ssz("Era file missing state slot index".to_string()))?;

        let mut group = if let Some(block_index) = block_slot_index {
            EraGroup::with_block_index(blocks, state, block_index, state_slot_index)
        } else {
            EraGroup::new(blocks, state, state_slot_index)
        };

        // Add other entries
        for entry in other_entries {
            group.add_entry(entry);
        }

        let start_slot = group.slot_index.as_ref().map(|idx| idx.starting_slot).unwrap_or(0);

        let slot_count = group.blocks.len() as u32;

        let id = EraId::new(network_name, start_slot, slot_count);

        Ok(EraFile::new(group, id))
    }
}

impl FileReader for EraReader<File> {}

/// Writer for Era files that builds on top of [`E2StoreWriter`]
#[derive(Debug)]
pub struct EraWriter<W: Write> {
    writer: E2StoreWriter<W>,
    has_written_version: bool,
    has_written_blocks: bool,
    has_written_state: bool,
    has_written_slot_indices: bool,
}

impl<W: Write> StreamWriter<W> for EraWriter<W> {
    type File = EraFile;

    /// Create a new [`EraWriter`]
    fn new(writer: W) -> Self {
        Self {
            writer: E2StoreWriter::new(writer),
            has_written_version: false,
            has_written_blocks: false,
            has_written_state: false,
            has_written_slot_indices: false,
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

    fn write_file(&mut self, file: &Self::File) -> Result<(), E2sError> {
        // Write version
        self.write_version()?;

        // Write all blocks
        for block in &file.group.blocks {
            self.write_block(block)?;
        }

        // Write state
        self.write_state(&file.group.era_state)?;

        // Write other entries
        for entry in &file.group.other_entries {
            self.writer.write_entry(entry)?;
        }

        // Write slot indices
        if let Some(ref block_index) = file.group.slot_index {
            self.write_slot_index(block_index)?;
        }
        self.write_slot_index(&file.group.state_slot_index)?;

        self.writer.flush()?;
        Ok(())
    }

    /// Flush any buffered data to the underlying writer
    fn flush(&mut self) -> Result<(), E2sError> {
        self.writer.flush()
    }
}

impl<W: Write> EraWriter<W> {
    /// Write for a single slot
    pub fn write_slot(&mut self, block_tuple: &BlockTuple) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
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

    /// Write the slot index
    pub fn write_slot_index(&mut self, slot_index: &SlotIndex) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        if self.has_written_slot_indices {
            return Err(E2sError::Ssz("Block index already written".to_string()));
        }

        let block_index_entry = slot_index.to_entry();
        self.writer.write_entry(&block_index_entry)?;
        self.has_written_slot_indices = true;

        Ok(())
    }
    fn write_state(&mut self, state: &CompressedBeaconState) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        if self.has_written_state {
            return Err(E2sError::Ssz("State already written".to_string()));
        }

        let entry = state.to_entry();
        self.writer.write_entry(&entry)?;
        self.has_written_state = true;
        Ok(())
    }

    /// Write a beacon block
    fn write_block(&mut self, block: &CompressedSignedBeaconBlock) -> Result<(), E2sError> {
        if !self.has_written_version {
            self.write_version()?;
        }

        let entry = block.to_entry();
        self.writer.write_entry(&entry)?;
        self.has_written_blocks = true;
        Ok(())
    }
}
