use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use thiserror::Error;
use zstd::bulk::Decompressor;

pub mod compression;
use compression::{Compression, Compressors};

const NIPPY_JAR_VERSION: usize = 1;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct NippyJar {
    /// Version
    version: usize,
    /// Number of columns
    columns: usize,
    /// Compressor if required
    compressor: Option<Compressors>,
    #[serde(skip)]
    /// Data path for file. Index file will be `{path}.idx`
    path: Option<PathBuf>,
    /// soon
    bloom_filter: bool,
    /// soon
    phf: bool,
}

impl NippyJar {
    /// Creates new [`NippyJar`].
    pub fn new(columns: usize, bloom_filter: bool, phf: bool, path: &Path) -> Self {
        NippyJar {
            version: NIPPY_JAR_VERSION,
            columns,
            bloom_filter,
            phf,
            compressor: None,
            path: Some(path.to_path_buf()),
        }
    }

    /// Adds [`compression::Zstd`] compression.
    pub fn with_zstd(mut self, use_dict: bool, max_dict_size: usize) -> Self {
        self.compressor =
            Some(Compressors::Zstd(compression::Zstd::new(use_dict, max_dict_size, self.columns)));
        self
    }

    /// Loads the file configuration and returns [`Self`].
    pub fn load(path: &Path) -> Result<Self, NippyJarError> {
        let mut file = File::open(path)?;
        let mut obj: Self = bincode::deserialize_from(&mut file)?;

        obj.path = Some(path.to_path_buf());
        Ok(obj)
    }

    /// Returns the path from the data file
    pub fn data_path(&self) -> PathBuf {
        self.path.clone().expect("exists")
    }

    /// Returns the path from the index file
    pub fn index_path(&self) -> PathBuf {
        let data_path = self.data_path();
        data_path
            .parent()
            .expect("exists")
            .join(format!("{}.idx", data_path.file_name().expect("exists").to_string_lossy()))
    }

    /// If required, prepares any compression algorithm to an early pass of the data.
    pub fn prepare(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        // Makes any necessary preparations for the compressors
        if let Some(compression) = &mut self.compressor {
            compression.prepare_compression(columns)?;
        }
        Ok(())
    }

    /// Writes all necessary configuration to file.
    pub fn freeze(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
        total_rows: u64,
    ) -> Result<(), NippyJarError> {
        let mut file = self.freeze_check(&columns)?;

        self.freeze_config(&mut file)?;

        // Special case for zstd that might use custom dictionaries/compressors per column
        // If any other compression algorithm is added and uses a similar flow, then revisit
        // implementation
        let mut maybe_zstd_compressors = None;
        if let Some(Compressors::Zstd(zstd)) = &self.compressor {
            maybe_zstd_compressors = zstd.generate_compressors()?;
        }

        // Temporary buffer to avoid multiple reallocations if compressing to a buffer (eg. zstd w/
        // dict)
        let mut tmp_buf = Vec::with_capacity(100);

        // Write all rows while taking all row start offsets
        let mut row_number = 0u64;
        let mut values_offsets = Vec::with_capacity(total_rows as usize * self.columns);
        let mut column_iterators =
            columns.into_iter().map(|v| v.into_iter()).collect::<Vec<_>>().into_iter();

        loop {
            let mut iterators = Vec::with_capacity(self.columns);

            // Write the column value of each row
            for (column_number, mut column_iter) in column_iterators.enumerate() {
                values_offsets.push(file.stream_position()? as usize);

                match column_iter.next() {
                    Some(value) => {
                        if let Some(compression) = &self.compressor {
                            // Special zstd case with dictionaries
                            if let (Some(dict_compressors), Compressors::Zstd(zstd)) =
                                (maybe_zstd_compressors.as_mut(), compression)
                            {
                                zstd.compress_with_dictionaries(
                                    &value,
                                    &mut tmp_buf,
                                    &mut file,
                                    Some(dict_compressors.get_mut(column_number).expect("exists")),
                                )?;
                            } else {
                                compression.compress_to(&value, &mut file)?;
                            }
                        } else {
                            file.write_all(&value)?;
                        }
                    }
                    None => {
                        return Err(NippyJarError::UnexpectedMissingValue(
                            row_number,
                            column_number as u64,
                        ))
                    }
                }

                iterators.push(column_iter);
            }

            row_number += 1;
            if row_number == total_rows {
                break
            }

            column_iterators = iterators.into_iter();
        }

        // Write offset index to file
        bincode::serialize_into(File::create(self.index_path())?, &values_offsets)?;

        Ok(())
    }

    /// Safety checks before creating and returning a [`File`] handle to write data to.
    fn freeze_check(
        &mut self,
        columns: &Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<File, NippyJarError> {
        if columns.len() != self.columns {
            return Err(NippyJarError::ColumnLenMismatch(self.columns, columns.len()))
        }

        if let Some(compression) = &self.compressor {
            if !compression.is_ready() {
                return Err(NippyJarError::CompressorNotReady)
            }
        }

        Ok(File::create(self.data_path())?)
    }

    /// Writes all necessary configuration to file.
    fn freeze_config(&self, handle: &mut File) -> Result<(), NippyJarError> {
        // TODO Split Dictionaries and Bloomfilters Configuration so we dont have to load everything
        // at once
        Ok(bincode::serialize_into(handle, &self)?)
    }
}

#[derive(Debug, Error)]
pub enum NippyJarError {
    #[error("err")]
    Err,
    #[error("err")]
    Disconnect(#[from] std::io::Error),
    #[error("err")]
    Bincode(#[from] Box<bincode::ErrorKind>),
    #[error("Compression was enabled, but it's not ready yet.")]
    CompressorNotReady,
    #[error("Decompression was enabled, but it's not ready yet.")]
    DecompressorNotReady,
    #[error("Number of columns does not match. {0} != {1}")]
    ColumnLenMismatch(usize, usize),
    #[error("UnexpectedMissingValue row: {0} col:{1}")]
    UnexpectedMissingValue(u64, u64),
}

pub struct NippyJarCursor<'a> {
    config: &'a NippyJar,
    zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    data_offsets: Vec<u64>,
    data_handle: File,
    row: u64,
    col: u64,
}

impl<'a> std::fmt::Debug for NippyJarCursor<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NippyJarCursor {{ config: {:?} }}", self.config)
    }
}

impl<'a> NippyJarCursor<'a> {
    pub fn new(
        config: &'a NippyJar,
        zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    ) -> Result<Self, NippyJarError> {
        let data_offsets = bincode::deserialize_from(File::open(config.index_path())?)?;

        Ok(NippyJarCursor {
            config,
            zstd_decompressors,
            data_offsets,
            data_handle: File::open(config.data_path())?,
            row: 0,
            col: 0,
        })
    }

    pub fn reset(&mut self) {
        self.row = 0;
        self.col = 0;
    }

    pub fn next_row(&mut self) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if self.row as usize * self.config.columns == self.data_offsets.len() {
            // Has reached the end
            return Ok(None)
        }

        let mut decompressed = Vec::with_capacity(32);
        let mut row = Vec::with_capacity(self.config.columns);
        let mut column_value = Vec::with_capacity(32);

        for column in 0..self.config.columns {
            let index_offset = self.row as usize * self.config.columns + column;
            let value_offset = self.data_offsets[index_offset];

            self.data_handle.seek(SeekFrom::Start(value_offset))?;

            // It's the last column of the last row
            if self.data_offsets.len() == (index_offset + 1) {
                column_value.clear();
                self.data_handle.read_to_end(&mut column_value)?;
            } else {
                let len = (self.data_offsets[index_offset + 1] - value_offset) as usize;
                column_value.resize(len, 0);
                self.data_handle.read_exact(&mut column_value[..len])?;
            }

            if let Some(zstd_dict_decompressors) = &self.zstd_decompressors {
                // Decompress using zstd dictionaries
                decompressed.clear();
                decompressed.reserve(column_value.len());

                zstd_dict_decompressors
                    .lock()
                    .unwrap()
                    .get_mut(column)
                    .unwrap()
                    .decompress_to_buffer(&column_value, &mut decompressed)?;

                row.push(decompressed.clone());
            } else if let Some(compression) = &self.config.compressor {
                // Decompress using the vanilla
                row.push(compression.decompress(&column_value)?);
            } else {
                // It's not compressed
                row.push(column_value.clone())
            }
        }

        if row.is_empty() {
            return Ok(None)
        }

        self.row += 1;

        Ok(Some(row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILE_NAME: &str = "nippyjar.nj";

    #[test]
    fn zstd() {
        let col1 = vec![
            vec![3, 0],
            vec![3, 1],
            vec![3, 2],
            vec![3, 3],
            vec![3, 4],
            vec![3, 5],
            vec![3, 6],
            vec![3, 7],
            vec![3, 8],
        ];
        let col2 = vec![
            vec![3, 10],
            vec![3, 11],
            vec![3, 12],
            vec![3, 13],
            vec![3, 14],
            vec![3, 15],
            vec![3, 16],
            vec![3, 17],
            vec![3, 18],
        ];
        let num_rows = col1.len() as u64;
        let _num_columns = 2;

        let data_file = NippyJar::new(
            _num_columns,
            false,
            false,
            &std::env::temp_dir().as_path().join(TEST_FILE_NAME),
        );

        assert_eq!(data_file.compressor, None);

        let mut data_file = NippyJar::new(
            _num_columns,
            false,
            false,
            &std::env::temp_dir().as_path().join(TEST_FILE_NAME),
        )
        .with_zstd(true, 5000);

        if let Some(Compressors::Zstd(zstd)) = &mut data_file.compressor {
            assert!(matches!(zstd.generate_compressors(), Err(NippyJarError::CompressorNotReady)));

            // Make sure the number of column iterators match the initial set up ones.
            assert!(matches!(
                zstd.prepare_compression(vec![col1.clone(), col2.clone(), col2.clone()]),
                Err(NippyJarError::ColumnLenMismatch(_num_columns, 3))
            ));
        } else {
            panic!("Expected ZSTD compressor");
        }

        let data = vec![col1, col2];

        // If ZSTD is enabled, do not write to the file unless the column dictionaries have been
        // calculated.
        assert!(matches!(
            data_file.freeze(data.clone(), num_rows),
            Err(NippyJarError::CompressorNotReady)
        ));

        data_file.prepare(data.clone()).unwrap();

        if let Some(Compressors::Zstd(zstd)) = &data_file.compressor {
            assert!(matches!(
                (&zstd.state, zstd.raw_dictionaries.as_ref().map(|dict| dict.len())),
                (compression::ZstdState::Ready, Some(_num_columns))
            ));
        }

        data_file.freeze(data.clone(), num_rows).unwrap();

        let written_data =
            NippyJar::load(&std::env::temp_dir().as_path().join(TEST_FILE_NAME)).unwrap();
        assert_eq!(data_file, written_data);

        if let Some(Compressors::Zstd(zstd)) = &data_file.compressor {
            let mut cursor = NippyJarCursor::new(
                &written_data,
                Some(Mutex::new(zstd.generate_decompressors().unwrap())),
            )
            .unwrap();

            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!((&row[0], &row[1]), (&data[0][row_index], &data[1][row_index]));
                row_index += 1;
            }
        }
    }
}
