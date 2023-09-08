use crate::NippyJarError;
use serde::{Deserialize, Serialize};
use std::io::Write;

mod zstd;
pub use zstd::{Zstd, ZstdState};

pub trait Compression {
    /// Returns decompressed data.
    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError>;

    /// Compresses data from `src` to `dest`
    fn compress_to<W: Write>(&self, src: &[u8], dest: &mut W) -> Result<(), NippyJarError>;

    /// Returns `true` if it's ready to compress.
    fn is_ready(&self) -> bool {
        true
    }

    /// Informs the compression algorithm that it was loaded from disk.
    fn was_loaded(&mut self) {}

    /// If required, prepares compression algorithm with an early pass on the data.
    fn prepare_compression(
        &mut self,
        _columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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

    fn is_ready(&self) -> bool {
        match self {
            Compressors::Zstd(zstd) => zstd.is_ready(),
            Compressors::Unused => unimplemented!(),
        }
    }

    fn was_loaded(&mut self) {
        match self {
            Compressors::Zstd(zstd) => zstd.was_loaded(),
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
