use thiserror::Error;

/// Errors associated with [`crate::NippyJar`].
#[derive(Debug, Error)]
pub enum NippyJarError {
    #[error(transparent)]
    Disconnect(#[from] std::io::Error),
    #[error(transparent)]
    Bincode(#[from] Box<bincode::ErrorKind>),
    #[error(transparent)]
    EliasFano(#[from] anyhow::Error),
    #[error("Compression was enabled, but it's not ready yet.")]
    CompressorNotReady,
    #[error("Decompression was enabled, but it's not ready yet.")]
    DecompressorNotReady,
    #[error("Number of columns does not match. {0} != {1}")]
    ColumnLenMismatch(usize, usize),
    #[error("UnexpectedMissingValue row: {0} col:{1}")]
    UnexpectedMissingValue(u64, u64),
    #[error(transparent)]
    FilterError(#[from] cuckoofilter::CuckooError),
    #[error("NippyJar initialized without filter.")]
    FilterMissing,
    #[error("Filter has reached max capacity.")]
    FilterMaxCapacity,
    #[error("Cuckoo was not properly initialized after loaded.")]
    FilterCuckooNotLoaded,
    #[error("Perfect hashing function doesn't have any keys added.")]
    PHFMissingKeys,
    #[error("NippyJar initialized without perfect hashing function.")]
    PHFMissing,
    #[error("NippyJar was built without an index.")]
    UnsupportedFilterQuery,
}
