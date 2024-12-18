use std::path::PathBuf;
use thiserror::Error;

/// Errors associated with [`crate::NippyJar`].
#[derive(Error, Debug)]
pub enum NippyJarError {
    /// An internal error occurred, wrapping any type of error.
    #[error(transparent)]
    Internal(#[from] Box<dyn core::error::Error + Send + Sync>),

    /// An error occurred while disconnecting, wrapping a standard I/O error.
    #[error(transparent)]
    Disconnect(#[from] std::io::Error),

    /// An error related to the file system occurred, wrapping a file system path error.
    #[error(transparent)]
    FileSystem(#[from] reth_fs_util::FsPathError),

    /// A custom error message provided by the user.
    #[error("{0}")]
    Custom(String),

    /// An error occurred during serialization/deserialization with Bincode.
    #[error(transparent)]
    Bincode(#[from] Box<bincode::ErrorKind>),

    /// An error occurred with the Elias-Fano encoding/decoding process.
    #[error(transparent)]
    EliasFano(#[from] anyhow::Error),

    /// Compression was enabled, but the compressor is not ready yet.
    #[error("compression was enabled, but it's not ready yet")]
    CompressorNotReady,

    /// Decompression was enabled, but the decompressor is not ready yet.
    #[error("decompression was enabled, but it's not ready yet")]
    DecompressorNotReady,

    /// The number of columns does not match the expected length.
    #[error("number of columns does not match: {0} != {1}")]
    ColumnLenMismatch(usize, usize),

    /// An unexpected missing value was encountered at a specific row and column.
    #[error("unexpected missing value: row:col {0}:{1}")]
    UnexpectedMissingValue(u64, u64),

    /// The size of an offset exceeds the maximum allowed size of 8 bytes.
    #[error("the size of an offset must be at most 8 bytes, got {offset_size}")]
    OffsetSizeTooBig {
        /// The read offset size in number of bytes.
        offset_size: u8,
    },

    /// The size of an offset is less than the minimum allowed size of 1 byte.
    #[error("the size of an offset must be at least 1 byte, got {offset_size}")]
    OffsetSizeTooSmall {
        /// The read offset size in number of bytes.
        offset_size: u8,
    },

    /// An attempt was made to read an offset that is out of bounds.
    #[error("attempted to read an out of bounds offset: {index}")]
    OffsetOutOfBounds {
        /// The index of the offset that was being read.
        index: usize,
    },

    /// The output buffer is too small for the compression or decompression operation.
    #[error("compression or decompression requires a bigger destination output")]
    OutputTooSmall,

    /// A dictionary is not loaded when it is required for operations.
    #[error("dictionary is not loaded.")]
    DictionaryNotLoaded,

    /// It's not possible to generate a compressor after loading a dictionary.
    #[error("it's not possible to generate a compressor after loading a dictionary.")]
    CompressorNotAllowed,

    /// The number of offsets is smaller than the requested prune size.
    #[error("number of offsets ({0}) is smaller than prune request ({1}).")]
    InvalidPruning(u64, u64),

    /// The jar has been frozen and cannot be modified.
    #[error("jar has been frozen and cannot be modified.")]
    FrozenJar,

    /// The file is in an inconsistent state.
    #[error("File is in an inconsistent state.")]
    InconsistentState,

    /// A specified file is missing.
    #[error("Missing file: {}", .0.display())]
    MissingFile(PathBuf),
}
