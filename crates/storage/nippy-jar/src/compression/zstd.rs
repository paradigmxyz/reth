use crate::{compression::Compression, NippyJarError};
use serde::{Deserialize, Serialize, Deserializer};
use std::{
    fs::File,
    io::{Read, Write},
};
use zstd::bulk::{Compressor, Decompressor};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ZstdState {
    #[default]
    PendingDictionary,
    Ready,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Zstd {
    #[serde(deserialize_with = "deserialize_state_as_ready")]
    pub(crate) state: ZstdState,
    pub(crate) use_dict: bool,
    pub(crate) max_dict_size: usize,
    pub(crate) raw_dictionaries: Option<Vec<Vec<u8>>>,
    columns: usize,
}

impl Zstd {
    pub fn new(use_dict: bool, max_dict_size: usize, columns: usize) -> Self {
        // todo add level
        Self {
            state: if use_dict { ZstdState::PendingDictionary } else { ZstdState::Ready },
            use_dict,
            max_dict_size,
            raw_dictionaries: None,
            columns,
        }
    }

    pub fn generate_decompressors(&self) -> Result<Vec<Decompressor<'_>>, NippyJarError> {
        if let Some(dictionaries) = &self.raw_dictionaries {
            return Ok((0..self.columns)
                .map(|column| Decompressor::with_dictionary(&dictionaries[column]))
                .collect::<Result<Vec<_>, _>>()?)
        }
        Ok(vec![])
    }

    pub fn generate_compressors(&self) -> Result<Option<Vec<Compressor>>, NippyJarError> {
        match self.state {
            ZstdState::PendingDictionary => Err(NippyJarError::CompressorNotReady),
            ZstdState::Ready => {
                if !self.use_dict {
                    return Ok(None)
                }

                let mut compressors = None;
                if let Some(dictionaries) = &self.raw_dictionaries {
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

    pub fn compress_with_dictionaries(
        &self,
        value: &[u8],
        tmp_buf: &mut Vec<u8>,
        handle: &mut File,
        compressor: Option<&mut Compressor>,
    ) -> Result<(), NippyJarError> {
        if let Some(compressor) = compressor {
            // Compressor requires the destination buffer to be big enough to write, otherwise it
            // fails. However, we don't know how big it will be. If data is small
            // enough, the compressed buffer will actually be larger. We keep retrying.
            // If we eventually fail, it probably means it's another kind of error.
            let mut multiplier = 1;
            while let Err(err) = compressor.compress_to_buffer(value, tmp_buf) {
                tmp_buf.reserve(value.len() * multiplier);
                multiplier += 1;
                if multiplier == 5 {
                    return Err(NippyJarError::Disconnect(err))
                }
            }

            handle.write_all(tmp_buf)?;
            tmp_buf.clear();
        } else {
            handle.write_all(value)?;
        }

        Ok(())
    }
}

impl Compression for Zstd {
    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        let mut decompressed = Vec::with_capacity(value.len() * 2);
        let mut decoder = zstd::Decoder::new(value)?;
        decoder.read_exact(&mut decompressed)?;
        Ok(decompressed)
    }
    fn compress_to<W: Write>(&self, src: &[u8], dest: &mut W) -> Result<(), NippyJarError> {
        let level = 0;

        let mut encoder = zstd::Encoder::new(dest, level)?;
        encoder.write_all(src)?;

        encoder.finish()?;

        Ok(())
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

        // There's a per 2GB hard limit on each column data training, iterator should take care of
        // it. Each element should ideally be bigger than 1 (otherwise sigev) huffman coding
        // + sueprstring ?

        // TODO select columns not to compress
        // TODO calculate max memory of machine before parallelizing

        if columns.len() != self.columns {
            return Err(NippyJarError::ColumnLenMismatch(self.columns, columns.len()))
        }

        // TODO parallel
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


fn deserialize_state_as_ready<'de, D>(deserializer: D) -> Result<ZstdState, D::Error>
where
    D: Deserializer<'de>,
{
    let _ = ZstdState::deserialize(deserializer)?;
    Ok(ZstdState::Ready)
}