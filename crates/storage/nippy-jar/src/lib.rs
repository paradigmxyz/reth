use serde::{Deserialize, Serialize};
use std::{
    clone::Clone,
    fs::File,
    hash::Hash,
    io::{Seek, Write},
    marker::Sync,
    path::{Path, PathBuf},
};
use sucds::{
    int_vectors::PrefixSummedEliasFano,
    mii_sequences::{EliasFano, EliasFanoBuilder},
    Serializable,
};

pub mod filter;
use filter::{Cuckoo, Filter, Filters};

pub mod compression;
use compression::{Compression, Compressors};

pub mod phf;
use phf::{Fmph, Functions, GoFmph, PerfectHashingFunction};

mod error;
pub use error::NippyJarError;

mod cursor;
pub use cursor::NippyJarCursor;

const NIPPY_JAR_VERSION: usize = 1;

/// A [`Row`] is a list of its selected column values.
type Row = Vec<Vec<u8>>;

/// Placeholder type for when a jar has no user header.
pub type NoJarHeader = bool;

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct NippyJar<H> {
    /// Version
    version: usize,
    /// User header data
    user_header: Option<H>,
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

impl<H> NippyJar<H>
where
    H: Send + Sync + Serialize + for<'a> Deserialize<'a>,
{
    /// Creates new [`NippyJar`].
    pub fn new(columns: usize, path: &Path) -> Self {
        NippyJar {
            version: NIPPY_JAR_VERSION,
            user_header: None,
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

    /// Adds a user header.
    pub fn with_user_header(mut self, data: H) -> Self {
        self.user_header = Some(data);
        self
    }

    /// Gets a reference to the user header.
    pub fn user_header(&self) -> &H {
        self.user_header.as_ref().expect("should exist")
    }

    /// Loads the file configuration and returns [`Self`].
    pub fn load(path: &Path) -> Result<Self, NippyJarError> {
        // Read [`Self`] located at the data file.
        let data_file = File::open(path)?;
        let data_reader = unsafe { memmap2::Mmap::map(&data_file)? };
        let mut obj: Self = bincode::deserialize_from(data_reader.as_ref())?;
        obj.path = Some(path.to_path_buf());

        // Read the offsets lists located at the index file.
        let offsets_file = File::open(obj.index_path())?;
        let mmap = unsafe { memmap2::Mmap::map(&offsets_file)? };
        let mut offsets_reader = mmap.as_ref();
        obj.offsets = EliasFano::deserialize_from(&mut offsets_reader)?;
        obj.offsets_index = PrefixSummedEliasFano::deserialize_from(offsets_reader)?;

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

    /// Prepares beforehand the offsets index for querying rows based on `values` (eg. transaction
    /// hash). Expects `values` to be sorted in the same way as the data that is going to be
    /// later on inserted.
    pub fn prepare_index<T: AsRef<[u8]> + Sync + Clone + Hash>(
        &mut self,
        values: &[T],
    ) -> Result<(), NippyJarError> {
        let mut offsets_index = vec![0; values.len()];

        // Builds perfect hashing function from the values
        if let Some(phf) = self.phf.as_mut() {
            phf.set_keys(values)?;
        }

        if self.filter.is_some() || self.phf.is_some() {
            for (row_num, v) in values.iter().enumerate() {
                if let Some(filter) = self.filter.as_mut() {
                    filter.add(v.as_ref())?;
                }

                if let Some(phf) = self.phf.as_mut() {
                    // Points to the first column value offset of the row.
                    let index = phf.get_index(v.as_ref())?.expect("initialized") as usize;
                    let _ = std::mem::replace(&mut offsets_index[index], row_num as u64);
                }
            }
        }

        self.offsets_index = PrefixSummedEliasFano::from_slice(&offsets_index)?;
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
                            if let (Some(dict_compressors), Compressors::Zstd(_)) =
                                (maybe_zstd_compressors.as_mut(), compression)
                            {
                                compression::Zstd::compress_with_dictionary(
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

        // Write offsets and offset index to file
        self.freeze_offsets(offsets)?;

        Ok(())
    }

    /// Freezes offsets and its own index.
    fn freeze_offsets(&mut self, offsets: Vec<usize>) -> Result<(), NippyJarError> {
        if !offsets.is_empty() {
            let mut builder =
                EliasFanoBuilder::new(*offsets.last().expect("qed") + 1, offsets.len())?;

            for offset in offsets {
                builder.push(offset)?;
            }
            self.offsets = builder.build().enable_rank();
        }
        let mut file = File::create(self.index_path())?;
        self.offsets.serialize_into(&mut file)?;
        self.offsets_index.serialize_into(file)?;
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

impl<H> Filter for NippyJar<H>
where
    H: Send + Sync + Serialize + for<'a> Deserialize<'a>,
{
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        self.filter.as_mut().ok_or(NippyJarError::FilterMissing)?.add(element)
    }

    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError> {
        self.filter.as_ref().ok_or(NippyJarError::FilterMissing)?.contains(element)
    }
}

impl<H> PerfectHashingFunction for NippyJar<H>
where
    H: Send + Sync + Serialize + for<'a> Deserialize<'a>,
{
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

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::SmallRng, seq::SliceRandom, RngCore, SeedableRng};
    use std::collections::HashSet;

    type ColumnValues = Vec<Vec<u8>>;

    fn test_data(seed: Option<u64>) -> (ColumnValues, ColumnValues) {
        let value_length = 32;
        let num_rows = 100;

        let mut vec: Vec<u8> = vec![0; value_length];
        let mut rng = seed.map(SmallRng::seed_from_u64).unwrap_or_else(SmallRng::from_entropy);

        let mut gen = || {
            (0..num_rows)
                .map(|_| {
                    rng.fill_bytes(&mut vec[..]);
                    vec.clone()
                })
                .collect()
        };

        (gen(), gen())
    }

    #[test]
    fn test_phf() {
        let (col1, col2) = test_data(None);
        let num_columns = 2;
        let num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let mut nippy = NippyJar::<NoJarHeader>::new(num_columns, file_path.path());
        assert!(matches!(NippyJar::set_keys(&mut nippy, &col1), Err(NippyJarError::PHFMissing)));

        let check_phf = |nippy: &mut NippyJar<_>| {
            assert!(matches!(
                NippyJar::get_index(nippy, &col1[0]),
                Err(NippyJarError::PHFMissingKeys)
            ));
            assert!(NippyJar::set_keys(nippy, &col1).is_ok());

            let collect_indexes = |nippy: &NippyJar<_>| -> Vec<u64> {
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
            let loaded_nippy = NippyJar::<NoJarHeader>::load(file_path.path()).unwrap();
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
    fn test_filter() {
        let (col1, col2) = test_data(Some(1));
        let _num_columns = 2;
        let num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let mut nippy = NippyJar::<NoJarHeader>::new(_num_columns, file_path.path());

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
        let loaded_nippy = NippyJar::<NoJarHeader>::load(file_path.path()).unwrap();

        assert_eq!(nippy, loaded_nippy);

        assert!(Filter::contains(&loaded_nippy, &col1[0]).unwrap());
        assert!(Filter::contains(&loaded_nippy, &col1[1]).unwrap());
        assert!(Filter::contains(&loaded_nippy, &col1[2]).unwrap());
        assert!(Filter::contains(&loaded_nippy, &col1[3]).unwrap());
        assert!(!Filter::contains(&loaded_nippy, &col1[4]).unwrap());
    }

    #[test]
    fn test_zstd_with_dictionaries() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let _num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let nippy = NippyJar::<NoJarHeader>::new(_num_columns, file_path.path());
        assert!(nippy.compressor.is_none());

        let mut nippy =
            NippyJar::<NoJarHeader>::new(_num_columns, file_path.path()).with_zstd(true, 5000);
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

        nippy.freeze(data.clone(), num_rows).unwrap();

        let mut loaded_nippy = NippyJar::<NoJarHeader>::load(file_path.path()).unwrap();
        assert_eq!(nippy, loaded_nippy);

        let mut dicts = vec![];
        if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_mut() {
            dicts = zstd.generate_decompress_dictionaries().unwrap()
        }

        if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_ref() {
            let mut cursor = NippyJarCursor::new(
                &loaded_nippy,
                Some(zstd.generate_decompressors(&dicts).unwrap()),
            )
            .unwrap();

            // Iterate over compressed values and compare
            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!((&row[0], &row[1]), (&data[0][row_index], &data[1][row_index]));
                row_index += 1;
            }
        }
    }

    #[test]
    fn test_zstd_no_dictionaries() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let _num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let nippy = NippyJar::<NoJarHeader>::new(_num_columns, file_path.path());
        assert!(nippy.compressor.is_none());

        let mut nippy =
            NippyJar::<NoJarHeader>::new(_num_columns, file_path.path()).with_zstd(false, 5000);
        assert!(nippy.compressor.is_some());

        let data = vec![col1.clone(), col2.clone()];

        nippy.freeze(data.clone(), num_rows).unwrap();

        let loaded_nippy = NippyJar::<NoJarHeader>::load(file_path.path()).unwrap();
        assert_eq!(nippy, loaded_nippy);

        if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_ref() {
            assert!(!zstd.use_dict);

            let mut cursor = NippyJarCursor::new(&loaded_nippy, None).unwrap();

            // Iterate over compressed values and compare
            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!((&row[0], &row[1]), (&data[0][row_index], &data[1][row_index]));
                row_index += 1;
            }
        } else {
            panic!("Expected Zstd compressor")
        }
    }

    /// Tests NippyJar with everything enabled: compression, filter, offset list and offset index.
    #[test]
    fn test_full_nippy_jar() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let _num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();
        let data = vec![col1.clone(), col2.clone()];

        // Create file
        {
            let mut nippy = NippyJar::<NoJarHeader>::new(_num_columns, file_path.path())
                .with_zstd(true, 5000)
                .with_cuckoo_filter(col1.len())
                .with_mphf();

            nippy.prepare_compression(data.clone()).unwrap();
            nippy.prepare_index(&col1).unwrap();
            nippy.freeze(data.clone(), num_rows).unwrap();
        }

        // Read file
        {
            let mut loaded_nippy = NippyJar::<NoJarHeader>::load(file_path.path()).unwrap();

            assert!(loaded_nippy.compressor.is_some());
            assert!(loaded_nippy.filter.is_some());
            assert!(loaded_nippy.phf.is_some());

            let mut dicts = vec![];
            if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_mut() {
                dicts = zstd.generate_decompress_dictionaries().unwrap()
            }
            if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_ref() {
                let mut cursor = NippyJarCursor::new(
                    &loaded_nippy,
                    Some(zstd.generate_decompressors(&dicts).unwrap()),
                )
                .unwrap();

                // Iterate over compressed values and compare
                let mut row_num = 0usize;
                while let Some(row) = cursor.next_row().unwrap() {
                    assert_eq!((&row[0], &row[1]), (&data[0][row_num], &data[1][row_num]));
                    row_num += 1;
                }

                // Shuffled for chaos.
                let mut data = col1.iter().zip(col2.iter()).enumerate().collect::<Vec<_>>();
                data.shuffle(&mut rand::thread_rng());

                for (row_num, (v0, v1)) in data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    let row_by_value = cursor.row_by_key(v0).unwrap().unwrap();
                    assert_eq!((&row_by_value[0], &row_by_value[1]), (v0, v1));

                    // Simulates `by_number` queries
                    let row_by_num = cursor.row_by_number(row_num).unwrap().unwrap();
                    assert_eq!(row_by_value, row_by_num);
                }
            }
        }
    }

    #[test]
    fn test_selectable_column_values() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let _num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();
        let data = vec![col1.clone(), col2.clone()];

        // Create file
        {
            let mut nippy = NippyJar::<NoJarHeader>::new(_num_columns, file_path.path())
                .with_zstd(true, 5000)
                .with_cuckoo_filter(col1.len())
                .with_mphf();

            nippy.prepare_compression(data.clone()).unwrap();
            nippy.prepare_index(&col1).unwrap();
            nippy.freeze(data.clone(), num_rows).unwrap();
        }

        // Read file
        {
            let mut loaded_nippy = NippyJar::<NoJarHeader>::load(file_path.path()).unwrap();

            let mut dicts = vec![];
            if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_mut() {
                dicts = zstd.generate_decompress_dictionaries().unwrap()
            }
            if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor.as_ref() {
                let mut cursor = NippyJarCursor::new(
                    &loaded_nippy,
                    Some(zstd.generate_decompressors(&dicts).unwrap()),
                )
                .unwrap();

                // Shuffled for chaos.
                let mut data = col1.iter().zip(col2.iter()).enumerate().collect::<Vec<_>>();
                data.shuffle(&mut rand::thread_rng());

                // Imagine `Blocks` snapshot file has two columns: `Block | StoredWithdrawals`
                const BLOCKS_FULL_MASK: usize = 0b11;
                const BLOCKS_COLUMNS: usize = 2;

                // Read both columns
                for (row_num, (v0, v1)) in &data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    let row_by_value = cursor
                        .row_by_key_with_cols::<BLOCKS_FULL_MASK, BLOCKS_COLUMNS>(v0)
                        .unwrap()
                        .unwrap();
                    assert_eq!((&row_by_value[0], &row_by_value[1]), (*v0, *v1));

                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols::<BLOCKS_FULL_MASK, BLOCKS_COLUMNS>(*row_num)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_value, row_by_num);
                }

                // Read first column only: `Block`
                const BLOCKS_BLOCK_MASK: usize = 0b01;
                for (row_num, (v0, _)) in &data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    let row_by_value = cursor
                        .row_by_key_with_cols::<BLOCKS_BLOCK_MASK, BLOCKS_COLUMNS>(v0)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_value.len(), 1);
                    assert_eq!(&row_by_value[0], *v0);

                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols::<BLOCKS_BLOCK_MASK, BLOCKS_COLUMNS>(*row_num)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_num.len(), 1);
                    assert_eq!(row_by_value, row_by_num);
                }

                // Read second column only: `Block`
                const BLOCKS_WITHDRAWAL_MASK: usize = 0b10;
                for (row_num, (v0, v1)) in &data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    let row_by_value = cursor
                        .row_by_key_with_cols::<BLOCKS_WITHDRAWAL_MASK, BLOCKS_COLUMNS>(v0)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_value.len(), 1);
                    assert_eq!(&row_by_value[0], *v1);

                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols::<BLOCKS_WITHDRAWAL_MASK, BLOCKS_COLUMNS>(*row_num)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_num.len(), 1);
                    assert_eq!(row_by_value, row_by_num);
                }

                // Read nothing
                const BLOCKS_EMPTY_MASK: usize = 0b00;
                for (row_num, (v0, _)) in &data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    assert!(cursor
                        .row_by_key_with_cols::<BLOCKS_EMPTY_MASK, BLOCKS_COLUMNS>(v0)
                        .unwrap()
                        .unwrap()
                        .is_empty());

                    // Simulates `by_number` queries
                    assert!(cursor
                        .row_by_number_with_cols::<BLOCKS_EMPTY_MASK, BLOCKS_COLUMNS>(*row_num)
                        .unwrap()
                        .unwrap()
                        .is_empty());
                }
            }
        }
    }
}
