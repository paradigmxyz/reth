use crate::NippyJarError;
use serde::{Deserialize, Serialize};
use std::io::Write;

mod zstd;
pub use self::zstd::{Zstd, ZstdState};

/// Trait that will compress column values
pub trait Compression: Serialize + for<'a> Deserialize<'a> {
    /// Returns decompressed data.
    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError>;

    /// Compresses data from `src` to `dest`
    fn compress_to<W: Write>(&self, src: &[u8], dest: &mut W) -> Result<(), NippyJarError>;

    /// Compresses data from `src`
    fn compress(&self, src: &[u8]) -> Result<Vec<u8>, NippyJarError>;

    /// Returns `true` if it's ready to compress.
    ///
    /// Example: it will return false, if `zstd` with dictionary is set, but wasn't generated.
    fn is_ready(&self) -> bool {
        true
    }

    /// If required, prepares compression algorithm with an early pass on the data.
    fn prepare_compression(
        &mut self,
        _columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        Ok(())
    }
}

/// Enum with different [`Compression`] types.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Compressors {
    Zstd(Zstd),
    // Avoids irrefutable let errors. Remove this after adding another one.
    Unused,
}

impl Compression for Compressors {
    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.decompress(value),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn compress_to<W: Write>(&self, src: &[u8], dest: &mut W) -> Result<(), NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.compress_to(src, dest),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn compress(&self, src: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.compress(src),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn is_ready(&self) -> bool {
        match self {
            Compressors::Zstd(zstd) => zstd.is_ready(),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn prepare_compression(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.prepare_compression(columns),
            Compressors::Unused => Ok(()),
        }
    }
}
