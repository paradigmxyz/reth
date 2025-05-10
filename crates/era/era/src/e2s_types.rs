//! Types to build e2store files
//! ie. with `.e2s` extension
//!
//! e2store file contains header and entry
//!
//! The [`Header`] is an 8-byte structure at the beginning of each record in the file
//!
//! An [`Entry`] is a complete record in the file, consisting of both a [`Header`] and its
//! associated data

use ssz_derive::{Decode, Encode};
use std::io::{self, Read, Write};
use thiserror::Error;

/// [`Version`] record: ['e', '2']
pub const VERSION: [u8; 2] = [0x65, 0x32];

/// Empty record
pub const EMPTY: [u8; 2] = [0x00, 0x00];

/// `SlotIndex` record: ['i', '2']
pub const SLOT_INDEX: [u8; 2] = [0x69, 0x32];

/// Error types for e2s file operations
#[derive(Error, Debug)]
pub enum E2sError {
    /// IO error during file operations
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// Error during SSZ encoding/decoding
    #[error("SSZ error: {0}")]
    Ssz(String),

    /// Reserved field in header not zero
    #[error("Reserved field in header not zero")]
    ReservedNotZero,

    /// Error during snappy compression
    #[error("Snappy compression error: {0}")]
    SnappyCompression(String),

    /// Error during snappy decompression
    #[error("Snappy decompression error: {0}")]
    SnappyDecompression(String),

    /// Error during RLP encoding/decoding
    #[error("RLP error: {0}")]
    Rlp(String),
}

/// Header for TLV records in e2store files
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct Header {
    /// Record type identifier
    pub header_type: [u8; 2],

    /// Length of data following the header
    pub length: u32,

    /// Reserved field, must be zero
    pub reserved: u16,
}

impl Header {
    /// Create a new header with the specified type and length
    pub const fn new(header_type: [u8; 2], length: u32) -> Self {
        Self { header_type, length, reserved: 0 }
    }

    /// Read header from a reader
    pub fn read<R: Read>(reader: &mut R) -> Result<Option<Self>, E2sError> {
        let mut header_bytes = [0u8; 8];
        match reader.read_exact(&mut header_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let header: Self = match ssz::Decode::from_ssz_bytes(&header_bytes) {
            Ok(h) => h,
            Err(_) => return Err(E2sError::Ssz(String::from("Failed to decode SSZ header"))),
        };

        if header.reserved != 0 {
            return Err(E2sError::ReservedNotZero);
        }

        Ok(Some(header))
    }

    /// Writes the header to the given writer.
    pub fn write<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let encoded = ssz::Encode::as_ssz_bytes(self);
        writer.write_all(&encoded)
    }
}

/// The [`Version`] record must be the first record in an e2store file
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Version;

impl Version {
    /// Encode this record to the given writer
    pub fn encode<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let header = Header::new(VERSION, 0);
        header.write(writer)
    }
}

/// Complete record in an e2store file, consisting of a type, length, and associated data
#[derive(Debug, Clone)]
pub struct Entry {
    /// Record type identifier
    pub entry_type: [u8; 2],

    /// Data contained in the entry
    pub data: Vec<u8>,
}

impl Entry {
    /// Create a new entry
    pub const fn new(entry_type: [u8; 2], data: Vec<u8>) -> Self {
        Self { entry_type, data }
    }

    /// Read an entry from a reader
    pub fn read<R: Read>(reader: &mut R) -> Result<Option<Self>, E2sError> {
        // Read the header first
        let header = match Header::read(reader)? {
            Some(h) => h,
            None => return Ok(None),
        };

        // Read the data
        let mut data = vec![0u8; header.length as usize];
        match reader.read_exact(&mut data) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(E2sError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Unexpected EOF while reading entry data",
                )));
            }
            Err(e) => return Err(e.into()),
        }

        Ok(Some(Self { entry_type: header.header_type, data }))
    }

    /// Write the entry to [`Entry`] writer
    pub fn write<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let header = Header::new(self.entry_type, self.data.len() as u32);
        header.write(writer)?;
        writer.write_all(&self.data)
    }

    /// Check if this is a [`Version`] entry
    pub fn is_version(&self) -> bool {
        self.entry_type == VERSION
    }

    /// Check if this is a `SlotIndex` entry
    pub fn is_slot_index(&self) -> bool {
        self.entry_type == SLOT_INDEX
    }
}
