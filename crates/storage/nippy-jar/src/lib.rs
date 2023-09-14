use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use thiserror::Error;
use zstd::bulk::Decompressor;

pub mod filter;
use filter::{Cuckoo, Filter, Filters};

pub mod compression;
use compression::{Compression, Compressors};

pub mod phf;
use phf::{Fmph, Functions};

const NIPPY_JAR_VERSION: usize = 1;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct NippyJar {
    /// Version
    version: usize,
    /// Number of columns
    columns: usize,
    /// Compressor if required
    compressor: Option<Compressors>,
    /// Filter
    filter: Option<Filters>,
    /// Perfect Hashing Function
    phf: Option<Functions>,
    #[serde(skip)]
    /// Data path for file. Index file will be `{path}.idx`
    path: Option<PathBuf>,
}

impl NippyJar {
    /// Creates new [`NippyJar`].
    pub fn new(columns: usize, path: &Path) -> Self {
        NippyJar {
            version: NIPPY_JAR_VERSION,
            columns,
            compressor: None,
            filter: None,
            phf: None,
            path: Some(path.to_path_buf()),
        }
    }

    /// Adds [`compression::Zstd`] compression.
    pub fn with_zstd(mut self, use_dict: bool, max_dict_size: usize) -> Self {
        self.compressor =
            Some(Compressors::Zstd(compression::Zstd::new(use_dict, max_dict_size, self.columns)));
        self
    }

    /// Adds [`filter::Cuckoo`] filter.
    pub fn with_cuckoo_filter(mut self, max_capacity: usize) -> Self {
        self.filter = Some(Filters::Cuckoo(Cuckoo::new(max_capacity)));
        self
    }

    /// Adds [`phf::Fmph`] perfect hashing function.
    pub fn with_mphf(mut self) -> Self {
        self.phf = Some(Functions::Fmph(Fmph::new()));
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

    /// Writes all data and configuration to a file and the offset index to another.
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
            // TODO: iter_mut if we remove the IntoIterator interface.
            for (column_number, mut column_iter) in column_iterators.enumerate() {
                values_offsets.push(file.stream_position()? as usize);

                match column_iter.next() {
                    Some(value) => {
                        if let Some(compression) = &self.compressor {
                            // Special zstd case with dictionaries
                            if let (Some(dict_compressors), Compressors::Zstd(zstd)) =
                                (maybe_zstd_compressors.as_mut(), compression)
                            {
                                zstd.compress_with_dictionary(
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
    fn freeze_config(&mut self, handle: &mut File) -> Result<(), NippyJarError> {
        // TODO Split Dictionaries and Bloomfilters Configuration so we dont have to load everything
        // at once
        Ok(bincode::serialize_into(handle, &self)?)
    }
}

impl Filter for NippyJar {
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        self.filter.as_mut().ok_or(NippyJarError::FilterMissing)?.add(element)
    }

    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError> {
        self.filter.as_ref().ok_or(NippyJarError::FilterMissing)?.contains(element)
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
    #[error("err")]
    FilterError(#[from] cuckoofilter::CuckooError),
    #[error("NippyJar initialized without filter.")]
    FilterMissing,
    #[error("Filter has reached max capacity.")]
    FilterMaxCapacity,
    #[error("Cuckoo was not properly initialized after loaded.")]
    FilterCuckooNotLoaded,
    #[error("Perfect hashing function wasn't added any keys.")]
    PHFMissingKeys,
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

    fn test_data() -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        ((0..10u8).map(|a| vec![2, a]).collect(), (10..20u8).map(|a| vec![3, a]).collect())
    }

    #[test]
    fn phf() {
        let (col1, _col2) = test_data();
        let _num_columns = 2;
        let _num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let _nippy = NippyJar::new(_num_columns, file_path.path());
    }

    #[test]
    fn filter() {
        let (col1, col2) = test_data();
        let _num_columns = 2;
        let num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let mut nippy = NippyJar::new(_num_columns, file_path.path());

        assert!(matches!(Filter::add(&mut nippy, &col1[0]), Err(NippyJarError::FilterMissing)));

        nippy = nippy.with_cuckoo_filter(4);

        // Add col1[0]
        assert!(!Filter::contains(&nippy, &col1[0]).unwrap());
        assert!(Filter::add(&mut nippy, &col1[0]).is_ok());
        assert!(Filter::contains(&nippy, &col1[0]).unwrap());

        // Add col1[1]
        assert!(!Filter::contains(&nippy, &col1[1]).unwrap());
        assert!(Filter::add(&mut nippy, &col1[1]).is_ok());
        assert!(Filter::contains(&nippy, &col1[1]).unwrap());

        // // Add more columns until max_capacity
        assert!(Filter::add(&mut nippy, &col1[2]).is_ok());
        assert!(Filter::add(&mut nippy, &col1[3]).is_ok());
        assert!(matches!(Filter::add(&mut nippy, &col1[4]), Err(NippyJarError::FilterMaxCapacity)));

        nippy.freeze(vec![col1.clone(), col2.clone()], num_rows).unwrap();
        let loaded_nippy = NippyJar::load(file_path.path()).unwrap();

        assert_eq!(nippy, loaded_nippy);

        assert!(Filter::contains(&loaded_nippy, &col1[0]).unwrap());
        assert!(Filter::contains(&loaded_nippy, &col1[1]).unwrap());
        assert!(Filter::contains(&loaded_nippy, &col1[2]).unwrap());
        assert!(Filter::contains(&loaded_nippy, &col1[3]).unwrap());
        assert!(!Filter::contains(&loaded_nippy, &col1[4]).unwrap());
    }

    #[test]
    fn zstd() {
        let (col1, col2) = test_data();
        let num_rows = col1.len() as u64;
        let _num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let nippy = NippyJar::new(_num_columns, file_path.path());
        assert!(nippy.compressor.is_none());

        let mut nippy = NippyJar::new(_num_columns, file_path.path()).with_zstd(true, 5000);
        assert!(nippy.compressor.is_some());

        if let Some(Compressors::Zstd(zstd)) = &mut nippy.compressor {
            assert!(matches!(zstd.generate_compressors(), Err(NippyJarError::CompressorNotReady)));

            // Make sure the number of column iterators match the initial set up ones.
            assert!(matches!(
                zstd.prepare_compression(vec![col1.clone(), col2.clone(), col2.clone()]),
                Err(NippyJarError::ColumnLenMismatch(_num_columns, 3))
            ));
        }

        let data = vec![col1, col2];

        // If ZSTD is enabled, do not write to the file unless the column dictionaries have been
        // calculated.
        assert!(matches!(
            nippy.freeze(data.clone(), num_rows),
            Err(NippyJarError::CompressorNotReady)
        ));

        nippy.prepare(data.clone()).unwrap();

        if let Some(Compressors::Zstd(zstd)) = &nippy.compressor {
            assert!(matches!(
                (&zstd.state, zstd.raw_dictionaries.as_ref().map(|dict| dict.len())),
                (compression::ZstdState::Ready, Some(_num_columns))
            ));
        }

        nippy.freeze(data.clone(), num_rows).unwrap();

        let loaded_nippy = NippyJar::load(file_path.path()).unwrap();
        assert_eq!(nippy, loaded_nippy);

        if let Some(Compressors::Zstd(zstd)) = &nippy.compressor {
            let mut cursor = NippyJarCursor::new(
                &loaded_nippy,
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
