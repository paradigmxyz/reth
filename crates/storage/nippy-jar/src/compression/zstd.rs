use crate::{compression::Compression, NippyJarError};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
};
use tracing::*;
use zstd::bulk::Compressor;
pub use zstd::{bulk::Decompressor, dict::DecoderDictionary};

type RawDictionary = Vec<u8>;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ZstdState {
    #[default]
    PendingDictionary,
    Ready,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
/// Zstd compression structure. Supports a compression dictionary per column.
pub struct Zstd {
    /// State. Should be ready before compressing.
    pub(crate) state: ZstdState,
    /// Compression level. A level of `0` uses zstd's default (currently `3`).
    pub(crate) level: i32,
    /// Uses custom dictionaries to compress data.
    pub use_dict: bool,
    /// Max size of a dictionary
    pub(crate) max_dict_size: usize,
    /// List of column dictionaries.
    pub(crate) raw_dictionaries: Option<Vec<RawDictionary>>,
    /// Number of columns to compress.
    columns: usize,
}

impl Zstd {
    /// Creates new [`Zstd`].
    pub fn new(use_dict: bool, max_dict_size: usize, columns: usize) -> Self {
        Self {
            state: if use_dict { ZstdState::PendingDictionary } else { ZstdState::Ready },
            level: 0,
            use_dict,
            max_dict_size,
            raw_dictionaries: None,
            columns,
        }
    }

    pub fn with_level(mut self, level: i32) -> Self {
        self.level = level;
        self
    }

    /// If using dictionaries, creates a list of [`DecoderDictionary`].
    ///
    /// Consumes `self.raw_dictionaries` in the process.
    pub fn generate_decompress_dictionaries<'a>(&mut self) -> Option<Vec<DecoderDictionary<'a>>> {
        self.raw_dictionaries.take().map(|dicts| {
            // TODO Can we use ::new instead, and avoid consuming?
            dicts.iter().map(|dict| DecoderDictionary::copy(dict)).collect()
        })
    }

    /// Creates a list of [`Decompressor`] using the given dictionaries.
    pub fn generate_decompressors<'a>(
        &self,
        dictionaries: &'a [DecoderDictionary<'a>],
    ) -> Result<Vec<Decompressor<'a>>, NippyJarError> {
        debug_assert!(dictionaries.len() == self.columns);

        Ok(dictionaries
            .iter()
            .map(Decompressor::with_prepared_dictionary)
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// If using dictionaries, creates a list of [`Compressor`].
    pub fn generate_compressors<'a>(&self) -> Result<Option<Vec<Compressor<'a>>>, NippyJarError> {
        match self.state {
            ZstdState::PendingDictionary => Err(NippyJarError::CompressorNotReady),
            ZstdState::Ready => {
                if !self.use_dict {
                    return Ok(None)
                }

                let mut compressors = None;
                if let Some(dictionaries) = &self.raw_dictionaries {
                    debug!(target: "nippy-jar", count=?dictionaries.len(), "Generating ZSTD compressor dictionaries.");

                    let mut cmp = Vec::with_capacity(dictionaries.len());

                    for dict in dictionaries {
                        cmp.push(Compressor::with_dictionary(0, dict)?);
                    }
                    compressors = Some(cmp)
                }
                Ok(compressors)
            }
        }
    }

    /// Compresses a value using a dictionary. Reserves additional capacity for `buffer` if
    /// necessary.
    pub fn compress_with_dictionary(
        column_value: &[u8],
        buffer: &mut Vec<u8>,
        handle: &mut File,
        compressor: Option<&mut Compressor<'_>>,
    ) -> Result<(), NippyJarError> {
        if let Some(compressor) = compressor {
            // Compressor requires the destination buffer to be big enough to write, otherwise it
            // fails. However, we don't know how big it will be. If data is small
            // enough, the compressed buffer will actually be larger. We keep retrying.
            // If we eventually fail, it probably means it's another kind of error.
            let mut multiplier = 1;
            while let Err(err) = compressor.compress_to_buffer(column_value, buffer) {
                buffer.reserve(column_value.len() * multiplier);
                multiplier += 1;
                if multiplier == 5 {
                    return Err(NippyJarError::Disconnect(err))
                }
            }

            handle.write_all(buffer)?;
            buffer.clear();
        } else {
            handle.write_all(column_value)?;
        }

        Ok(())
    }

    /// Appends a decompressed value using a dictionary to a user provided buffer.
    pub fn decompress_with_dictionary(
        column_value: &[u8],
        output: &mut Vec<u8>,
        decompressor: &mut Decompressor<'_>,
    ) -> Result<(), NippyJarError> {
        let previous_length = output.len();

        // SAFETY: We're setting len to the existing capacity.
        unsafe {
            output.set_len(output.capacity());
        }

        match decompressor.decompress_to_buffer(column_value, &mut output[previous_length..]) {
            Ok(written) => {
                // SAFETY: `decompress_to_buffer` can only write if there's enough capacity.
                // Therefore, it shouldn't write more than our capacity.
                unsafe {
                    output.set_len(previous_length + written);
                }
                Ok(())
            }
            Err(_) => {
                // SAFETY: we are resetting it to the previous value.
                unsafe {
                    output.set_len(previous_length);
                }
                Err(NippyJarError::OutputTooSmall)
            }
        }
    }
}

impl Compression for Zstd {
    fn decompress_to(&self, value: &[u8], dest: &mut Vec<u8>) -> Result<(), NippyJarError> {
        let mut decoder = zstd::Decoder::with_dictionary(value, &[])?;
        decoder.read_to_end(dest)?;
        Ok(())
    }

    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        let mut decompressed = Vec::with_capacity(value.len() * 2);
        let mut decoder = zstd::Decoder::new(value)?;
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }

    fn compress_to(&self, src: &[u8], dest: &mut Vec<u8>) -> Result<usize, NippyJarError> {
        let before = dest.len();

        let mut encoder = zstd::Encoder::new(dest, self.level)?;
        encoder.write_all(src)?;

        let dest = encoder.finish()?;

        Ok(dest.len() - before)
    }

    fn compress(&self, src: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        let mut compressed = Vec::with_capacity(src.len());

        self.compress_to(src, &mut compressed)?;

        Ok(compressed)
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ZstdState::Ready)
    }

    /// If using it with dictionaries, prepares a dictionary for each column.
    fn prepare_compression(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        if !self.use_dict {
            return Ok(())
        }

        // There's a per 2GB hard limit on each column data set for training
        // REFERENCE: https://github.com/facebook/zstd/blob/dev/programs/zstd.1.md#dictionary-builder
        // ```
        // -M#, --memory=#: Limit the amount of sample data loaded for training (default: 2 GB).
        // Note that the default (2 GB) is also the maximum. This parameter can be useful in
        // situations where the training set size is not well controlled and could be potentially
        // very large. Since speed of the training process is directly correlated to the size of the
        // training sample set, a smaller sample set leads to faster training.`
        // ```

        if columns.len() != self.columns {
            return Err(NippyJarError::ColumnLenMismatch(self.columns, columns.len()))
        }

        // TODO: parallel calculation
        let mut dictionaries = vec![];
        for column in columns {
            // ZSTD requires all training data to be continuous in memory, alongside the size of
            // each entry
            let mut sizes = vec![];
            let data: Vec<_> = column
                .into_iter()
                .flat_map(|data| {
                    sizes.push(data.len());
                    data
                })
                .collect();

            dictionaries.push(zstd::dict::from_continuous(&data, &sizes, self.max_dict_size)?);
        }

        debug_assert_eq!(dictionaries.len(), self.columns);

        self.raw_dictionaries = Some(dictionaries);
        self.state = ZstdState::Ready;

        Ok(())
    }
}
