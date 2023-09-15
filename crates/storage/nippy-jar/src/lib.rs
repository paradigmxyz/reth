use serde::{Deserialize, Serialize};
use std::{
    clone::Clone,
    fs::File,
    hash::Hash,
    io::{Read, Seek, SeekFrom, Write},
    marker::Sync,
    path::{Path, PathBuf},
    sync::Mutex,
};
use sucds::{
    int_vectors::{Access, PrefixSummedEliasFano},
    mii_sequences::{EliasFano, EliasFanoBuilder},
    Serializable,
};
use thiserror::Error;
use zstd::bulk::Decompressor;

pub mod filter;
use filter::{Cuckoo, Filter, Filters};

pub mod compression;
use compression::{Compression, Compressors};

pub mod phf;
use phf::{Fmph, Functions, GoFmph, KeySet};

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
    /// Indexes PHF output to the value offset at `self.offsets`
    offsets_index: PrefixSummedEliasFano,
    /// Values offsets eg. `[row0_col0_offset, row0_col1_offset, row1_col1_offset, row1,
    /// col2_offset ... ]` TODO: currently on a different file, but might be unnecessary
    #[serde(skip)]
    offsets: EliasFano,
    /// Data path for file. Index file will be `{path}.idx`
    #[serde(skip)]
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
            offsets: EliasFano::default(),
            offsets_index: PrefixSummedEliasFano::default(),
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

    /// Adds [`phf::GoFmph`] perfect hashing function.
    pub fn with_gomphf(mut self) -> Self {
        self.phf = Some(Functions::GoFmph(GoFmph::new()));
        self
    }

    /// Loads the file configuration and returns [`Self`].
    pub fn load(path: &Path) -> Result<Self, NippyJarError> {
        let mut file = File::open(path)?;
        let mut obj: Self = bincode::deserialize_from(&mut file)?;

        obj.path = Some(path.to_path_buf());

        let mut offsets_file = File::open(obj.index_path())?;
        obj.offsets = EliasFano::deserialize_from(&mut offsets_file).unwrap();
        obj.offsets_index = PrefixSummedEliasFano::deserialize_from(offsets_file).unwrap();

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
    pub fn prepare_compression(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        // Makes any necessary preparations for the compressors
        if let Some(compression) = &mut self.compressor {
            compression.prepare_compression(columns)?;
        }
        Ok(())
    }

    /// If required, prepares any compression algorithm to an early pass of the data.
    pub fn prepare_index<T: AsRef<[u8]> + Sync + Clone + Hash + std::fmt::Debug>(
        &mut self,
        values: &[T],
    ) -> Result<(), NippyJarError> {
        let mut offsets_index = vec![0; values.len()];

        if let Some(phf) = self.phf.as_mut() {
            phf.set_keys(values)?;
        }

        if self.filter.is_some() || self.phf.is_some() {
            for (row_num, v) in values.iter().enumerate() {
                // Point to the first column value of the row

                if let Some(filter) = self.filter.as_mut() {
                    filter.add(v.as_ref())?;
                }

                if let Some(phf) = self.phf.as_mut() {
                    let index = phf.get_index(v.as_ref())?.expect("initialized") as usize;
                    let _ = std::mem::replace(&mut offsets_index[index], row_num as u64);
                }
            }
        }

        self.offsets_index = PrefixSummedEliasFano::from_slice(&offsets_index).unwrap();
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
        let mut offsets = Vec::with_capacity(total_rows as usize * self.columns);
        let mut column_iterators =
            columns.into_iter().map(|v| v.into_iter()).collect::<Vec<_>>().into_iter();

        loop {
            let mut iterators = Vec::with_capacity(self.columns);

            // Write the column value of each row
            // TODO: iter_mut if we remove the IntoIterator interface.
            for (column_number, mut column_iter) in column_iterators.enumerate() {
                offsets.push(file.stream_position()? as usize);

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
        if !offsets.is_empty() {
            let mut builder =
                EliasFanoBuilder::new(*offsets.last().expect("qed") + 1, offsets.len()).unwrap();

            for offset in offsets {
                builder.push(offset).unwrap();
            }
            self.offsets = builder.build().enable_rank();
        }

        let mut f = File::create(self.index_path())?;
        self.offsets.serialize_into(&mut f).unwrap();
        self.offsets_index.serialize_into(f).unwrap();

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

        // Check `prepare_index` was called.
        if let Some(phf) = &self.phf {
            let _ = phf.get_index(&[])?;
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

impl KeySet for NippyJar {
    fn set_keys<T: AsRef<[u8]> + Sync + Clone + Hash>(
        &mut self,
        keys: &[T],
    ) -> Result<(), NippyJarError> {
        self.phf.as_mut().ok_or(NippyJarError::PHFMissing)?.set_keys(keys)
    }

    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError> {
        self.phf.as_ref().ok_or(NippyJarError::PHFMissing)?.get_index(key)
    }
}

#[derive(Debug, Error)]
pub enum NippyJarError {
    #[error(transparent)]
    Disconnect(#[from] std::io::Error),
    #[error(transparent)]
    Bincode(#[from] Box<bincode::ErrorKind>),
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
    #[error("Perfect hashing function wasn't added any keys.")]
    PHFMissingKeys,
    #[error("NippyJar initialized without perfect hashing function.")]
    PHFMissing,
}

pub struct NippyJarCursor<'a> {
    config: &'a NippyJar,
    zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    data_handle: File,
    tmp_buf: Vec<u8>,
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
        Ok(NippyJarCursor {
            config,
            zstd_decompressors,
            data_handle: File::open(config.data_path())?,
            tmp_buf: vec![],
            row: 0,
            col: 0,
        })
    }

    pub fn reset(&mut self) {
        self.row = 0;
        self.col = 0;
    }

    // eg. transaction_by_hash
    // may have false positives
    pub fn row_by_filter(&mut self, value: &[u8]) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if let (Some(filter), Some(phf)) = (&self.config.filter, &self.config.phf) {
            // May have false positives
            if filter.contains(value)? {
                // May have false positives
                let row_index = phf.get_index(value)?.unwrap();

                self.row = self.config.offsets_index.access(row_index as usize).unwrap() as u64;
                return self.next_row()
            }
        } else {
            panic!("requires it");
        }

        panic!("didnt find it");
    }

    // eg. transaction_by_nnumber
    pub fn row_by_number(&mut self, row: usize) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        self.row = row as u64;
        self.next_row()
    }

    pub fn next_row(&mut self) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if self.row as usize * self.config.columns == self.config.offsets.len() {
            // Has reached the end
            return Ok(None)
        }

        let mut row = Vec::with_capacity(self.config.columns);
        let mut column_value = Vec::with_capacity(32);

        for column in 0..self.config.columns {
            let index_offset = self.row as usize * self.config.columns + column;
            let value_offset = self.config.offsets.select(index_offset).unwrap();

            self.data_handle.seek(SeekFrom::Start(value_offset as u64))?;

            // It's the last column of the last row
            if self.config.offsets.len() == (index_offset + 1) {
                column_value.clear();
                self.data_handle.read_to_end(&mut column_value)?;
            } else {
                let len = self.config.offsets.select(index_offset + 1).unwrap() - value_offset;
                column_value.resize(len, 0);
                self.data_handle.read_exact(&mut column_value[..len])?;
            }

            if let Some(zstd_dict_decompressors) = &self.zstd_decompressors {
                // Decompress using zstd dictionaries
                let extra_capacity =
                    (column_value.len() * 2).saturating_sub(self.tmp_buf.capacity());
                self.tmp_buf.clear();
                self.tmp_buf.reserve(extra_capacity);

                zstd_dict_decompressors
                    .lock()
                    .unwrap()
                    .get_mut(column)
                    .unwrap()
                    .decompress_to_buffer(&column_value, &mut self.tmp_buf)?;

                row.push(self.tmp_buf.clone());
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
    use rand::{seq::SliceRandom, RngCore};

    use super::*;
    use std::collections::HashSet;

    fn test_data() -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let mut rng = rand::thread_rng();
        let value_size = 32;
        let num_rows = 100;
        let mut vec: Vec<u8> = vec![0; value_size];

        (
            (0..num_rows)
                .map(|_| {
                    rng.fill_bytes(&mut vec[..]);
                    vec.clone()
                })
                .collect(),
            (0..num_rows)
                .map(|_| {
                    rng.fill_bytes(&mut vec[..]);
                    vec.clone()
                })
                .collect(),
        )
    }

    #[test]
    fn phf() {
        let (col1, col2) = test_data();
        let num_columns = 2;
        let num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let mut nippy = NippyJar::new(num_columns, file_path.path());
        assert!(matches!(NippyJar::set_keys(&mut nippy, &col1), Err(NippyJarError::PHFMissing)));

        let check_phf = |nippy: &mut NippyJar| {
            assert!(matches!(
                NippyJar::get_index(nippy, &col1[0]),
                Err(NippyJarError::PHFMissingKeys)
            ));
            assert!(NippyJar::set_keys(nippy, &col1).is_ok());

            let collect_indexes = |nippy: &NippyJar| -> Vec<u64> {
                col1.iter()
                    .map(|value| NippyJar::get_index(nippy, value.as_slice()).unwrap().unwrap())
                    .collect()
            };

            // Ensure all indexes are unique
            let indexes = collect_indexes(nippy);
            assert_eq!(indexes.iter().collect::<HashSet<_>>().len(), indexes.len());

            // Ensure reproducibility
            assert!(NippyJar::set_keys(nippy, &col1).is_ok());
            assert_eq!(indexes, collect_indexes(nippy));

            // Ensure that loaded phf provides the same function outputs
            nippy.prepare_index(&col1).unwrap();
            nippy.freeze(vec![col1.clone(), col2.clone()], num_rows).unwrap();
            let loaded_nippy = NippyJar::load(file_path.path()).unwrap();
            assert_eq!(indexes, collect_indexes(&loaded_nippy));
        };

        // mphf bytes size for 100 values of 32 bytes: 54
        nippy = nippy.with_mphf();
        check_phf(&mut nippy);

        // mphf bytes size for 100 values of 32 bytes: 46
        nippy = nippy.with_gomphf();
        check_phf(&mut nippy);
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

    /// Tests NippyJar with everything enabled. Zstd, filter and
    #[test]
    fn test_full_nippy_jar() {
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

        let data = vec![col1.clone(), col2.clone()];

        // If ZSTD is enabled, do not write to the file unless the column dictionaries have been
        // calculated.
        assert!(matches!(
            nippy.freeze(data.clone(), num_rows),
            Err(NippyJarError::CompressorNotReady)
        ));

        nippy.prepare_compression(data.clone()).unwrap();

        if let Some(Compressors::Zstd(zstd)) = &nippy.compressor {
            assert!(matches!(
                (&zstd.state, zstd.raw_dictionaries.as_ref().map(|dict| dict.len())),
                (compression::ZstdState::Ready, Some(_num_columns))
            ));
        }
        nippy = nippy.with_cuckoo_filter(col1.len());
        nippy = nippy.with_mphf();
        nippy.prepare_index(&col1).unwrap();
        nippy.freeze(data.clone(), num_rows).unwrap();

        let loaded_nippy = NippyJar::load(file_path.path()).unwrap();
        assert_eq!(nippy, loaded_nippy);

        if let Some(Compressors::Zstd(zstd)) = &nippy.compressor {
            let mut cursor = NippyJarCursor::new(
                &loaded_nippy,
                Some(Mutex::new(zstd.generate_decompressors().unwrap())),
            )
            .unwrap();

            // Iterate over compressed values and compare
            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!((&row[0], &row[1]), (&data[0][row_index], &data[1][row_index]));
                row_index += 1;
            }

            // Shuffled for chaos.
            let mut data = col1.iter().zip(col2.iter()).enumerate().collect::<Vec<_>>();
            data.shuffle(&mut rand::thread_rng());

            for (row_num, (v0, v1)) in data {
                // Simulates `by_hash` queries by iterating col1 values, which were used to create
                // the inner index.
                let row_by_value = cursor.row_by_filter(v0).unwrap().unwrap();
                assert_eq!((&row_by_value[0], &row_by_value[1]), (v0, v1));

                // Simulates `by_number` queries
                let row_by_num = cursor.row_by_number(row_num).unwrap().unwrap();
                assert_eq!(row_by_value, row_by_num);
            }
        }
    }
}
