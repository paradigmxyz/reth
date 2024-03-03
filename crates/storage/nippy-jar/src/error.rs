use thiserror::Error;

/// Errors associated with [`crate::NippyJar`].
#[derive(Error, Debug)]
pub enum NippyJarError {
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Disconnect(#[from] std::io::Error),
    #[error(transparent)]
    FileSystem(#[from] reth_primitives::fs::FsPathError),
    #[error("{0}")]
    Custom(String),
    #[error(transparent)]
    Bincode(#[from] Box<bincode::ErrorKind>),
    #[error(transparent)]
    EliasFano(#[from] anyhow::Error),
    #[error("compression was enabled, but it's not ready yet")]
    CompressorNotReady,
    #[error("decompression was enabled, but it's not ready yet")]
    DecompressorNotReady,
    #[error("number of columns does not match: {0} != {1}")]
    ColumnLenMismatch(usize, usize),
    #[error("unexpected missing value: row:col {0}:{1}")]
    UnexpectedMissingValue(u64, u64),
    #[error(transparent)]
    FilterError(#[from] cuckoofilter::CuckooError),
    #[error("nippy jar initialized without filter")]
    FilterMissing,
    #[error("filter has reached max capacity")]
    FilterMaxCapacity,
    #[error("cuckoo was not properly initialized after loaded")]
    FilterCuckooNotLoaded,
    #[error("perfect hashing function doesn't have any keys added")]
    PHFMissingKeys,
    #[error("nippy jar initialized without perfect hashing function")]
    PHFMissing,
    #[error("nippy jar was built without an index")]
    UnsupportedFilterQuery,
    #[error("compression or decompression requires a bigger destination output")]
    OutputTooSmall,
    #[error("dictionary is not loaded.")]
    DictionaryNotLoaded,
    #[error("it's not possible to generate a compressor after loading a dictionary.")]
    CompressorNotAllowed,
    #[error("number of offsets ({0}) is smaller than prune request ({1}).")]
    InvalidPruning(u64, u64),
    #[error("jar has been frozen and cannot be modified.")]
    FrozenJar,
}
