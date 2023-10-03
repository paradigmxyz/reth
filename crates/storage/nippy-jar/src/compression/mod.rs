use crate::NippyJarError;
use serde::{Deserialize, Serialize};

mod zstd;
pub use self::zstd::{DecoderDictionary, Decompressor, Zstd, ZstdState};
mod lz4;
pub use self::lz4::Lz4;

/// Trait that will compress column values
pub trait Compression: Serialize + for<'a> Deserialize<'a> {
    /// Appends decompressed data to the dest buffer. `dest` should be allocated with enough memory.
    fn decompress_to(&self, value: &[u8], dest: &mut Vec<u8>) -> Result<(), NippyJarError>;

    /// Returns decompressed data.
    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError>;

    /// Appends compressed data from `src` to `dest`. `dest` should be allocated with enough memory.
    fn compress_to(&self, src: &[u8], dest: &mut Vec<u8>) -> Result<usize, NippyJarError>;

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
    Lz4(Lz4),
    // Avoids irrefutable let errors. Remove this after adding another one.
    Unused,
}

impl Compression for Compressors {
    fn decompress_to(&self, value: &[u8], dest: &mut Vec<u8>) -> Result<(), NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.decompress_to(value, dest),
            Compressors::Lz4(lz4) => lz4.decompress_to(value, dest),
            Compressors::Unused => unimplemented!(),
        }
    }
    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.decompress(value),
            Compressors::Lz4(lz4) => lz4.decompress(value),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn compress_to(&self, src: &[u8], dest: &mut Vec<u8>) -> Result<usize, NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.compress_to(src, dest),
            Compressors::Lz4(lz4) => lz4.compress_to(src, dest),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn compress(&self, src: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.compress(src),
            Compressors::Lz4(lz4) => lz4.compress(src),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn is_ready(&self) -> bool {
        match self {
            Compressors::Zstd(zstd) => zstd.is_ready(),
            Compressors::Lz4(lz4) => lz4.is_ready(),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn prepare_compression(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        match self {
            Compressors::Zstd(zstd) => zstd.prepare_compression(columns),
            Compressors::Lz4(lz4) => lz4.prepare_compression(columns),
            Compressors::Unused => Ok(()),
        }
    }
}
