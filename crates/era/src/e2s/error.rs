//! Error handling for e2s files operations

use std::io;
use thiserror::Error;

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
