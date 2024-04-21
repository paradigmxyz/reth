use crate::{compression::Compression, NippyJarError};
use derive_more::Deref;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fs::File,
    io::{Read, Write},
    sync::Arc,
};
use tracing::*;
use zstd::bulk::Compressor;
pub use zstd::{bulk::Decompressor, dict::DecoderDictionary};

type RawDictionary = Vec<u8>;

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZstdState {
    #[default]
    PendingDictionary,
    Ready,
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Debug, Serialize, Deserialize)]
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
    #[serde(with = "dictionaries_serde")]
    pub(crate) dictionaries: Option<Arc<ZstdDictionaries<'static>>>,
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
            dictionaries: None,
            columns,
        }
    }

    pub fn with_level(mut self, level: i32) -> Self {
        self.level = level;
        self
    }

    /// Creates a list of [`Decompressor`] if using dictionaries.
    pub fn decompressors(&self) -> Result<Vec<Decompressor<'_>>, NippyJarError> {
        if let Some(dictionaries) = &self.dictionaries {
            debug_assert!(dictionaries.len() == self.columns);
            return dictionaries.decompressors()
        }

        Ok(vec![])
    }

    /// If using dictionaries, creates a list of [`Compressor`].
    pub fn compressors(&self) -> Result<Option<Vec<Compressor<'_>>>, NippyJarError> {
        match self.state {
            ZstdState::PendingDictionary => Err(NippyJarError::CompressorNotReady),
            ZstdState::Ready => {
                if !self.use_dict {
                    return Ok(None)
                }

                if let Some(dictionaries) = &self.dictionaries {
                    debug!(target: "nippy-jar", count=?dictionaries.len(), "Generating ZSTD compressor dictionaries.");
                    return Ok(Some(dictionaries.compressors()?))
                }
                Ok(None)
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

        self.dictionaries = Some(Arc::new(ZstdDictionaries::new(dictionaries)));
        self.state = ZstdState::Ready;

        Ok(())
    }
}

mod dictionaries_serde {
    use super::*;

    pub(crate) fn serialize<S>(
        dictionaries: &Option<Arc<ZstdDictionaries<'static>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match dictionaries {
            Some(dicts) => serializer.serialize_some(dicts.as_ref()),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<Arc<ZstdDictionaries<'static>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let dictionaries: Option<Vec<RawDictionary>> = Option::deserialize(deserializer)?;
        Ok(dictionaries.map(|dicts| Arc::new(ZstdDictionaries::load(dicts))))
    }
}

/// List of [`ZstdDictionary`]
#[cfg_attr(test, derive(PartialEq))]
#[derive(Serialize, Deserialize, Deref)]
pub(crate) struct ZstdDictionaries<'a>(Vec<ZstdDictionary<'a>>);

impl<'a> std::fmt::Debug for ZstdDictionaries<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdDictionaries").field("num", &self.len()).finish_non_exhaustive()
    }
}

impl<'a> ZstdDictionaries<'a> {
    /// Creates [`ZstdDictionaries`].
    pub(crate) fn new(raw: Vec<RawDictionary>) -> Self {
        Self(raw.into_iter().map(ZstdDictionary::Raw).collect())
    }

    /// Loads a list [`RawDictionary`] into a list of [`ZstdDictionary::Loaded`].
    pub(crate) fn load(raw: Vec<RawDictionary>) -> Self {
        Self(
            raw.into_iter()
                .map(|dict| ZstdDictionary::Loaded(DecoderDictionary::copy(&dict)))
                .collect(),
        )
    }

    /// Creates a list of decompressors from a list of [`ZstdDictionary::Loaded`].
    pub(crate) fn decompressors(&self) -> Result<Vec<Decompressor<'_>>, NippyJarError> {
        Ok(self
            .iter()
            .flat_map(|dict| {
                dict.loaded()
                    .ok_or(NippyJarError::DictionaryNotLoaded)
                    .map(Decompressor::with_prepared_dictionary)
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Creates a list of compressors from a list of [`ZstdDictionary::Raw`].
    pub(crate) fn compressors(&self) -> Result<Vec<Compressor<'_>>, NippyJarError> {
        Ok(self
            .iter()
            .flat_map(|dict| {
                dict.raw()
                    .ok_or(NippyJarError::CompressorNotAllowed)
                    .map(|dict| Compressor::with_dictionary(0, dict))
            })
            .collect::<Result<Vec<_>, _>>()?)
    }
}

/// A Zstd dictionary. It's created and serialized with [`ZstdDictionary::Raw`], and deserialized as
/// [`ZstdDictionary::Loaded`].
pub(crate) enum ZstdDictionary<'a> {
    Raw(RawDictionary),
    Loaded(DecoderDictionary<'a>),
}

impl<'a> ZstdDictionary<'a> {
    /// Returns a reference to the expected `RawDictionary`
    pub(crate) fn raw(&self) -> Option<&RawDictionary> {
        match self {
            ZstdDictionary::Raw(dict) => Some(dict),
            ZstdDictionary::Loaded(_) => None,
        }
    }

    /// Returns a reference to the expected `DecoderDictionary`
    pub(crate) fn loaded(&self) -> Option<&DecoderDictionary<'_>> {
        match self {
            ZstdDictionary::Raw(_) => None,
            ZstdDictionary::Loaded(dict) => Some(dict),
        }
    }
}

impl<'de, 'a> Deserialize<'de> for ZstdDictionary<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let dict = RawDictionary::deserialize(deserializer)?;
        Ok(Self::Loaded(DecoderDictionary::copy(&dict)))
    }
}

impl<'a> Serialize for ZstdDictionary<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ZstdDictionary::Raw(r) => r.serialize(serializer),
            ZstdDictionary::Loaded(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
impl<'a> PartialEq for ZstdDictionary<'a> {
    fn eq(&self, other: &Self) -> bool {
        if let (Self::Raw(a), Self::Raw(b)) = (self, &other) {
            return a == b
        }
        unimplemented!("`DecoderDictionary` can't be compared. So comparison should be done after decompressing a value.");
    }
}
