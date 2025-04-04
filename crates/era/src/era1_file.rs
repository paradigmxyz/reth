//! Represents a complete Era1 file
//!
//! The structure of an Era1 file follows the specification:
//! `Version | block-tuple* | other-entries* | Accumulator | BlockIndex`
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

use crate::{
    e2s_file::E2StoreReader,
    e2s_types::{E2sError, Version, BLOCK_INDEX},
    era1_types::{BlockIndex, Era1Group, Era1Id},
    execution_types::{
        self, Accumulator, BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts,
        TotalDifficulty,
    },
};
use alloy_primitives::BlockNumber;
use std::{
    fs::File,
    io::{Read, Seek},
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
    pub fn new(group: Era1Group, id: Era1Id) -> Self {
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

impl<R: Read + Seek> Era1Reader<R> {
    /// Create a new [`Era1Reader`]
    pub fn new(reader: R) -> Self {
        Self { reader: E2StoreReader::new(reader) }
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

        // Read all entries
        let entries = self.reader.entries()?;

        // Skip the first entry since it's the version
        let entries = entries.into_iter().skip(1).collect::<Vec<_>>();

        // Temporary storage for block components
        let mut headers = Vec::new();
        let mut bodies = Vec::new();
        let mut receipts = Vec::new();
        let mut difficulties = Vec::new();
        let mut other_entries = Vec::new();
        let mut accumulator = None;
        let mut block_index = None;

        // Process entries in the order they appear
        for entry in entries {
            match entry.entry_type {
                execution_types::COMPRESSED_HEADER => {
                    headers.push(CompressedHeader::from_entry(&entry)?);
                }
                execution_types::COMPRESSED_BODY => {
                    bodies.push(CompressedBody::from_entry(&entry)?);
                }
                execution_types::COMPRESSED_RECEIPTS => {
                    receipts.push(CompressedReceipts::from_entry(&entry)?);
                }
                execution_types::TOTAL_DIFFICULTY => {
                    difficulties.push(TotalDifficulty::from_entry(&entry)?);
                }
                execution_types::ACCUMULATOR => {
                    if accumulator.is_some() {
                        return Err(E2sError::Ssz("Multiple accumulator entries found".to_string()));
                    }
                    accumulator = Some(Accumulator::from_entry(&entry)?);
                }
                BLOCK_INDEX => {
                    if block_index.is_some() {
                        return Err(E2sError::Ssz("Multiple block index entries found".to_string()));
                    }
                    block_index = Some(BlockIndex::from_entry(&entry)?);
                }
                _ => {
                    other_entries.push(entry);
                }
            }
        }

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

        let mut blocks = Vec::with_capacity(headers.len());
        for i in 0..headers.len() {
            blocks.push(BlockTuple::new(
                headers[i].clone(),
                bodies[i].clone(),
                receipts[i].clone(),
                difficulties[i].clone(),
            ));
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
