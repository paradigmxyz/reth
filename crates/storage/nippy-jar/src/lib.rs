//! Immutable data store format.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
// TODO(danipopes): add these warnings
// #![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::{
    error::Error as StdError,
    fs::File,
    io::{Seek, Write},
    marker::Sync,
    ops::Range,
    path::{Path, PathBuf},
};
use sucds::{int_vectors::PrefixSummedEliasFano, Serializable};
use tracing::*;

pub mod filter;
use filter::{Cuckoo, InclusionFilter, InclusionFilters};

pub mod compression;
use compression::{Compression, Compressors};

pub mod phf;
pub use phf::PHFKey;
use phf::{Fmph, Functions, GoFmph, PerfectHashingFunction};

mod error;
pub use error::NippyJarError;

mod cursor;
pub use cursor::NippyJarCursor;

const NIPPY_JAR_VERSION: usize = 1;

/// A [`RefRow`] is a list of column value slices pointing to either an internal buffer or a
/// memory-mapped file.
type RefRow<'a> = Vec<&'a [u8]>;

/// Alias type for a column value wrapped in `Result`
pub type ColumnResult<T> = Result<T, Box<dyn StdError + Send + Sync>>;

/// `NippyJar` is a specialized storage format designed for immutable data.
///
/// Data is organized into a columnar format, enabling column-based compression. Data retrieval
/// entails consulting an offset list and fetching the data from file via `mmap`.
///
/// PHF & Filters:
/// For data membership verification, the `filter` field can be configured with algorithms like
/// Bloom or Cuckoo filters. While these filters enable rapid membership checks, it's important to
/// note that **they may yield false positives but not false negatives**. Therefore, they serve as
/// preliminary checks (eg. in `by_hash` queries) and should be followed by data verification on
/// retrieval.
///
/// The `phf` (Perfect Hashing Function) and `offsets_index` fields facilitate the data retrieval
/// process in for example `by_hash` queries. Specifically, the PHF converts a query, such as a
/// block hash, into a unique integer. This integer is then used as an index in `offsets_index`,
/// which maps to the actual data location in the `offsets` list. Similar to the `filter`, the PHF
/// may also produce false positives but not false negatives, necessitating subsequent data
/// verification.
///
/// Note: that the key (eg. BlockHash) passed to a filter and phf does not need to actually be
/// stored.
///
/// Ultimately, the `freeze` function yields two files: a data file containing both the data and its
/// configuration, and an index file that houses the offsets and offsets_index.
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct NippyJar<H = ()> {
    /// The version of the NippyJar format.
    version: usize,
    /// User-defined header data.
    /// Default: zero-sized unit type: no header data
    user_header: H,
    /// Number of data columns in the jar.
    columns: usize,
    /// Number of data rows in the jar.
    rows: usize,
    /// Optional compression algorithm applied to the data.
    compressor: Option<Compressors>,
    #[serde(skip)]
    /// Optional filter function for data membership checks.
    filter: Option<InclusionFilters>,
    #[serde(skip)]
    /// Optional Perfect Hashing Function (PHF) for unique offset mapping.
    phf: Option<Functions>,
    /// Index mapping PHF output to value offsets in `offsets`.
    #[serde(skip)]
    offsets_index: PrefixSummedEliasFano,
    /// Maximum uncompressed row size of the set. This will enable decompression without any
    /// resizing of the output buffer.
    #[serde(skip)]
    max_row_size: usize,
    /// Data path for file. Index file will be `{path}.idx`
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl<H: std::fmt::Debug> std::fmt::Debug for NippyJar<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NippyJar")
            .field("version", &self.version)
            .field("user_header", &self.user_header)
            .field("columns", &self.columns)
            .field("compressor", &self.compressor)
            .field("filter", &self.filter)
            .field("phf", &self.phf)
            .field("offsets_index (len)", &self.offsets_index.len())
            .field("offsets_index (size in bytes)", &self.offsets_index.size_in_bytes())
            .field("path", &self.path)
            .field("max_row_size", &self.max_row_size)
            .finish_non_exhaustive()
    }
}

impl NippyJar<()> {
    /// Creates a new [`NippyJar`] without an user-defined header data.
    pub fn new_without_header(columns: usize, path: &Path) -> Self {
        NippyJar::<()>::new(columns, path, ())
    }

    /// Loads the file configuration and returns [`Self`] on a jar without user-defined header data.
    pub fn load_without_header(path: &Path) -> Result<Self, NippyJarError> {
        NippyJar::<()>::load(path)
    }

    /// Whether this [`NippyJar`] uses a [`InclusionFilters`] and [`Functions`].
    pub fn uses_filters(&self) -> bool {
        self.filter.is_some() && self.phf.is_some()
    }
}

impl<H> NippyJar<H>
where
    H: Send + Sync + Serialize + for<'a> Deserialize<'a> + std::fmt::Debug,
{
    /// Creates a new [`NippyJar`] with a user-defined header data.
    pub fn new(columns: usize, path: &Path, user_header: H) -> Self {
        NippyJar {
            version: NIPPY_JAR_VERSION,
            user_header,
            columns,
            rows: 0,
            max_row_size: 0,
            compressor: None,
            filter: None,
            phf: None,
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

    /// Adds [`compression::Lz4`] compression.
    pub fn with_lz4(mut self) -> Self {
        self.compressor = Some(Compressors::Lz4(compression::Lz4::default()));
        self
    }

    /// Adds [`filter::Cuckoo`] filter.
    pub fn with_cuckoo_filter(mut self, max_capacity: usize) -> Self {
        self.filter = Some(InclusionFilters::Cuckoo(Cuckoo::new(max_capacity)));
        self
    }

    /// Adds [`phf::Fmph`] perfect hashing function.
    pub fn with_fmph(mut self) -> Self {
        self.phf = Some(Functions::Fmph(Fmph::new()));
        self
    }

    /// Adds [`phf::GoFmph`] perfect hashing function.
    pub fn with_gofmph(mut self) -> Self {
        self.phf = Some(Functions::GoFmph(GoFmph::new()));
        self
    }

    /// Gets a reference to the user header.
    pub fn user_header(&self) -> &H {
        &self.user_header
    }

    /// Gets a reference to `self.offsets`.
    pub fn filter_size(&self) -> usize {
        self.size()
    }

    /// Gets a reference to `self.offsets_index`.
    pub fn offsets_index_size(&self) -> usize {
        self.offsets_index.size_in_bytes()
    }

    /// Gets a reference to the compressor.
    pub fn compressor(&self) -> Option<&Compressors> {
        self.compressor.as_ref()
    }

    /// Gets a mutable reference to the compressor.
    pub fn compressor_mut(&mut self) -> Option<&mut Compressors> {
        self.compressor.as_mut()
    }

    /// Loads the file configuration and returns [`Self`] without deserializing filters related
    /// structures or the offset list.
    ///
    /// **The user must ensure the header type matches the one used during the jar's creation.**
    pub fn load(path: &Path) -> Result<Self, NippyJarError> {
        // Read [`Self`] located at the data file.
        let data_file = File::open(path)?;

        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let data_reader = unsafe { memmap2::Mmap::map(&data_file)? };
        let max_row_size: [u8; 8] =
            data_reader.as_ref()[0..8].try_into().expect("slice with incorrect length");
        let max_row_size = u64::from_le_bytes(max_row_size);

        let mut obj: Self = bincode::deserialize_from(&data_reader.as_ref()[8..])?;
        obj.path = Some(path.to_path_buf());
        obj.max_row_size = max_row_size as usize;
        Ok(obj)
    }

    /// Loads filters into memory
    pub fn load_filters(mut self) -> Result<Self, NippyJarError> {
        // Read the offsets lists located at the index file.
        let offsets_file = File::open(self.index_path())?;

        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let mmap = unsafe { memmap2::Mmap::map(&offsets_file)? };
        let mut offsets_reader = mmap.as_ref();
        self.offsets_index = PrefixSummedEliasFano::deserialize_from(&mut offsets_reader)?;
        self.phf = bincode::deserialize_from(&mut offsets_reader)?;
        self.filter = bincode::deserialize_from(&mut offsets_reader)?;
        Ok(self)
    }

    /// Returns the path from the data file
    pub fn data_path(&self) -> &Path {
        self.path.as_ref().expect("exists")
    }

    /// Returns the path from the index file
    pub fn index_path(&self) -> PathBuf {
        self.data_path().parent().expect("exists").join(format!(
            "{}.idx",
            self.data_path().file_name().expect("exists").to_string_lossy()
        ))
    }

    /// Returns the path from the offsets file
    pub fn offsets_path(&self) -> PathBuf {
        self.data_path().parent().expect("exists").join(format!(
            "{}.off",
            self.data_path().file_name().expect("exists").to_string_lossy()
        ))
    }

    /// Returns a [`DataReader`] of the data and offset file
    pub fn open_data(&self) -> Result<DataReader, NippyJarError> {
        DataReader::new(self.data_path())
    }

    /// If required, prepares any compression algorithm to an early pass of the data.
    pub fn prepare_compression(
        &mut self,
        columns: Vec<impl IntoIterator<Item = Vec<u8>>>,
    ) -> Result<(), NippyJarError> {
        // Makes any necessary preparations for the compressors
        if let Some(compression) = &mut self.compressor {
            debug!(target: "nippy-jar", columns=columns.len(), "Preparing compression.");
            compression.prepare_compression(columns)?;
        }
        Ok(())
    }

    /// Prepares beforehand the offsets index for querying rows based on `values` (eg. transaction
    /// hash). Expects `values` to be sorted in the same way as the data that is going to be
    /// later on inserted.
    ///
    /// Currently collecting all items before acting on them.
    pub fn prepare_index<T: PHFKey>(
        &mut self,
        values: impl IntoIterator<Item = ColumnResult<T>>,
        row_count: usize,
    ) -> Result<(), NippyJarError> {
        debug!(target: "nippy-jar", ?row_count, "Preparing index.");

        let values = values.into_iter().collect::<Result<Vec<_>, _>>()?;

        debug_assert!(
            row_count == values.len(),
            "Row count ({row_count}) differs from value list count ({}).",
            values.len()
        );

        let mut offsets_index = vec![0; row_count];

        // Builds perfect hashing function from the values
        if let Some(phf) = self.phf.as_mut() {
            debug!(target: "nippy-jar", ?row_count, values_count = ?values.len(), "Setting keys for perfect hashing function.");
            phf.set_keys(&values)?;
        }

        if self.filter.is_some() || self.phf.is_some() {
            debug!(target: "nippy-jar", ?row_count, "Creating filter and offsets_index.");

            for (row_num, v) in values.into_iter().enumerate() {
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

        debug!(target: "nippy-jar", ?row_count, "Encoding offsets index list.");
        self.offsets_index = PrefixSummedEliasFano::from_slice(&offsets_index)?;
        Ok(())
    }

    /// Writes all data and configuration to a file and the offset index to another.
    pub fn freeze(
        &mut self,
        columns: Vec<impl IntoIterator<Item = ColumnResult<Vec<u8>>>>,
        total_rows: u64,
    ) -> Result<(), NippyJarError> {
        self.rows = total_rows as usize;

        let mut file = self.freeze_check(&columns)?;
        self.freeze_config(&mut file)?;

        // Special case for zstd that might use custom dictionaries/compressors per column
        // If any other compression algorithm is added and uses a similar flow, then revisit
        // implementation
        let mut maybe_zstd_compressors = None;
        if let Some(Compressors::Zstd(zstd)) = &self.compressor {
            maybe_zstd_compressors = zstd.compressors()?;
        }

        // Temporary buffer to avoid multiple reallocations if compressing to a buffer (eg. zstd w/
        // dict)
        let mut tmp_buf = Vec::with_capacity(1_000_000);

        // Write all rows while taking all row start offsets
        let mut row_number = 0u64;
        let mut offsets = Vec::with_capacity(total_rows as usize * self.columns);
        let mut column_iterators =
            columns.into_iter().map(|v| v.into_iter()).collect::<Vec<_>>().into_iter();

        debug!(target: "nippy-jar", compressor=?self.compressor, "Writing rows.");

        loop {
            let mut iterators = Vec::with_capacity(self.columns);

            // Write the column value of each row
            // TODO: iter_mut if we remove the IntoIterator interface.
            let mut uncompressed_row_size = 0;
            for (column_number, mut column_iter) in column_iterators.enumerate() {
                offsets.push(file.stream_position()?);

                match column_iter.next() {
                    Some(Ok(value)) => {
                        uncompressed_row_size += value.len();

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
                                let before = tmp_buf.len();
                                let len = compression.compress_to(&value, &mut tmp_buf)?;
                                file.write_all(&tmp_buf[before..before + len])?;
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
                    Some(Err(err)) => return Err(err.into()),
                }

                iterators.push(column_iter);
            }

            tmp_buf.clear();
            row_number += 1;
            self.max_row_size = self.max_row_size.max(uncompressed_row_size);

            if row_number == total_rows {
                break
            }

            column_iterators = iterators.into_iter();
        }

        // drops immutable borrow
        drop(maybe_zstd_compressors);

        // Write offsets and offset index to file
        self.freeze_offsets()?;

        debug!(target: "nippy-jar", jar=?self, "Finished.");

        Ok(())
    }

    /// Freezes offsets and its own index.
    fn freeze_offsets(&mut self) -> Result<(), NippyJarError> {
        debug!(target: "nippy-jar", path=?self.index_path(), "Writing offsets and offsets index to file.");

        let mut file = File::create(self.index_path())?;
        self.offsets_index.serialize_into(&mut file)?;
        bincode::serialize_into(&mut file, &self.phf)?;
        bincode::serialize_into(&mut file, &self.filter)?;

        Ok(())
    }

    /// Safety checks before creating and returning a [`File`] handle to write data to.
    fn freeze_check(
        &mut self,
        columns: &Vec<impl IntoIterator<Item = ColumnResult<Vec<u8>>>>,
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

        debug!(target: "nippy-jar", path=?self.data_path(), "Opening data file.");

        Ok(File::create(self.data_path())?)
    }

    /// Writes all necessary configuration to file.
    fn freeze_config(&mut self, handle: &mut File) -> Result<(), NippyJarError> {
        // TODO Split Dictionaries and Bloomfilters Configuration so we dont have to load everything
        // at once

        // Placeholder for max_row_size
        handle.write_all(&[0; 8])?;

        Ok(bincode::serialize_into(handle, &self)?)
    }
}

impl<H> InclusionFilter for NippyJar<H>
where
    H: Send + Sync + Serialize + for<'a> Deserialize<'a>,
{
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        self.filter.as_mut().ok_or(NippyJarError::FilterMissing)?.add(element)
    }

    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError> {
        self.filter.as_ref().ok_or(NippyJarError::FilterMissing)?.contains(element)
    }

    fn size(&self) -> usize {
        self.filter.as_ref().map(|f| f.size()).unwrap_or(0)
    }
}

impl<H> PerfectHashingFunction for NippyJar<H>
where
    H: Send + Sync + Serialize + for<'a> Deserialize<'a>,
{
    fn set_keys<T: PHFKey>(&mut self, keys: &[T]) -> Result<(), NippyJarError> {
        self.phf.as_mut().ok_or(NippyJarError::PHFMissing)?.set_keys(keys)
    }

    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError> {
        self.phf.as_ref().ok_or(NippyJarError::PHFMissing)?.get_index(key)
    }
}

/// Manages the reading of snapshot data using memory-mapped files.
///
/// Holds file and mmap descriptors of the data and offsets files of a snapshot.
#[derive(Debug)]
pub struct DataReader {
    /// Data file descriptor. Needs to be kept alive as long as the respective mmap handle.
    #[allow(unused)]
    data_file: File,
    /// Mmap handle for data.
    data_mmap: Mmap,
    /// Offset file descriptor. Needs to be kept alive as long as the respective mmap handle.
    #[allow(unused)]
    offset_file: File,
    /// Mmap handle for offsets.
    offset_mmap: Mmap,
    /// Number of bytes that represents one offset.
    offset_len: usize,
}

impl DataReader {
    /// Reads the respective data and offsets file and returns [`DataReader`].
    pub fn new(path: impl AsRef<Path>) -> Result<Self, NippyJarError> {
        let data_file = File::open(path.as_ref())?;
        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let data_mmap = unsafe { Mmap::map(&data_file)? };

        let offset_file = File::open(PathBuf::from(format!("{}.off", path.as_ref().display())))?;
        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let offset_mmap = unsafe { Mmap::map(&offset_file)? };

        Ok(Self {
            data_file,
            data_mmap,
            offset_file,
            // First byte indicates how many bytes represent each offset
            offset_len: offset_mmap[0] as usize,
            offset_mmap,
        })
    }

    /// Returns the offset for the requested data index
    pub fn offset(&self, index: usize) -> u64 {
        let mut buffer: [u8; 8] = [0; 8];

        // + 1 represents the offset_len u8 which is in the beginning of the file
        let from = index * self.offset_len + 1;
        let to = from + self.offset_len;

        buffer[..self.offset_len].copy_from_slice(&self.offset_mmap[from..to]);
        u64::from_le_bytes(buffer)
    }

    /// Provides the underlying data as a slice on the provided offset range.
    pub fn data(&self, range: Range<usize>) -> &[u8] {
        &self.data_mmap[range]
    }

    /// Returns total size of data
    pub fn size(&self) -> usize {
        self.data_mmap.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::SmallRng, seq::SliceRandom, RngCore, SeedableRng};
    use std::collections::HashSet;

    type ColumnResults<T> = Vec<ColumnResult<T>>;
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

    fn clone_with_result(col: &ColumnValues) -> ColumnResults<Vec<u8>> {
        col.iter().map(|v| Ok(v.clone())).collect()
    }

    #[test]
    fn test_phf() {
        let (col1, col2) = test_data(None);
        let num_columns = 2;
        let num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let mut nippy = NippyJar::new_without_header(num_columns, file_path.path());
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
            nippy.prepare_index(clone_with_result(&col1), col1.len()).unwrap();
            nippy
                .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
                .unwrap();
            let loaded_nippy =
                NippyJar::load_without_header(file_path.path()).unwrap().load_filters().unwrap();
            assert_eq!(indexes, collect_indexes(&loaded_nippy));
        };

        // fmph bytes size for 100 values of 32 bytes: 54
        nippy = nippy.with_fmph();
        check_phf(&mut nippy);

        // fmph bytes size for 100 values of 32 bytes: 46
        nippy = nippy.with_gofmph();
        check_phf(&mut nippy);
    }

    #[test]
    fn test_filter() {
        let (col1, col2) = test_data(Some(1));
        let num_columns = 2;
        let num_rows = col1.len() as u64;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let mut nippy = NippyJar::new_without_header(num_columns, file_path.path());

        assert!(matches!(
            InclusionFilter::add(&mut nippy, &col1[0]),
            Err(NippyJarError::FilterMissing)
        ));

        nippy = nippy.with_cuckoo_filter(4);

        // Add col1[0]
        assert!(!InclusionFilter::contains(&nippy, &col1[0]).unwrap());
        assert!(InclusionFilter::add(&mut nippy, &col1[0]).is_ok());
        assert!(InclusionFilter::contains(&nippy, &col1[0]).unwrap());

        // Add col1[1]
        assert!(!InclusionFilter::contains(&nippy, &col1[1]).unwrap());
        assert!(InclusionFilter::add(&mut nippy, &col1[1]).is_ok());
        assert!(InclusionFilter::contains(&nippy, &col1[1]).unwrap());

        // // Add more columns until max_capacity
        assert!(InclusionFilter::add(&mut nippy, &col1[2]).is_ok());
        assert!(InclusionFilter::add(&mut nippy, &col1[3]).is_ok());

        nippy.freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows).unwrap();
        let loaded_nippy =
            NippyJar::load_without_header(file_path.path()).unwrap().load_filters().unwrap();

        assert_eq!(nippy, loaded_nippy);

        assert!(InclusionFilter::contains(&loaded_nippy, &col1[0]).unwrap());
        assert!(InclusionFilter::contains(&loaded_nippy, &col1[1]).unwrap());
        assert!(InclusionFilter::contains(&loaded_nippy, &col1[2]).unwrap());
        assert!(InclusionFilter::contains(&loaded_nippy, &col1[3]).unwrap());
        assert!(!InclusionFilter::contains(&loaded_nippy, &col1[4]).unwrap());
    }

    #[test]
    fn test_zstd_with_dictionaries() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let nippy = NippyJar::new_without_header(num_columns, file_path.path());
        assert!(nippy.compressor().is_none());

        let mut nippy =
            NippyJar::new_without_header(num_columns, file_path.path()).with_zstd(true, 5000);
        assert!(nippy.compressor().is_some());

        if let Some(Compressors::Zstd(zstd)) = &mut nippy.compressor_mut() {
            assert!(matches!(zstd.compressors(), Err(NippyJarError::CompressorNotReady)));

            // Make sure the number of column iterators match the initial set up ones.
            assert!(matches!(
                zstd.prepare_compression(vec![col1.clone(), col2.clone(), col2.clone()]),
                Err(NippyJarError::ColumnLenMismatch(columns, 3)) if columns == num_columns
            ));
        }

        // If ZSTD is enabled, do not write to the file unless the column dictionaries have been
        // calculated.
        assert!(matches!(
            nippy.freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows),
            Err(NippyJarError::CompressorNotReady)
        ));

        nippy.prepare_compression(vec![col1.clone(), col2.clone()]).unwrap();

        if let Some(Compressors::Zstd(zstd)) = &nippy.compressor() {
            assert!(matches!(
                (&zstd.state, zstd.dictionaries.as_ref().map(|dict| dict.len())),
                (compression::ZstdState::Ready, Some(columns)) if columns == num_columns
            ));
        }

        nippy.freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows).unwrap();

        let loaded_nippy =
            NippyJar::load_without_header(file_path.path()).unwrap().load_filters().unwrap();
        assert_eq!(nippy.version, loaded_nippy.version);
        assert_eq!(nippy.columns, loaded_nippy.columns);
        assert_eq!(nippy.filter, loaded_nippy.filter);
        assert_eq!(nippy.phf, loaded_nippy.phf);
        assert_eq!(nippy.offsets_index, loaded_nippy.offsets_index);
        assert_eq!(nippy.max_row_size, loaded_nippy.max_row_size);
        assert_eq!(nippy.path, loaded_nippy.path);

        if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor() {
            assert!(zstd.use_dict);
            let mut cursor = NippyJarCursor::new(&loaded_nippy).unwrap();

            // Iterate over compressed values and compare
            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!(
                    (row[0], row[1]),
                    (col1[row_index].as_slice(), col2[row_index].as_slice())
                );
                row_index += 1;
            }
        } else {
            panic!("Expected Zstd compressor")
        }
    }

    #[test]
    fn test_lz4() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let nippy = NippyJar::new_without_header(num_columns, file_path.path());
        assert!(nippy.compressor().is_none());

        let mut nippy = NippyJar::new_without_header(num_columns, file_path.path()).with_lz4();
        assert!(nippy.compressor().is_some());

        nippy.freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows).unwrap();

        let loaded_nippy = NippyJar::load_without_header(file_path.path()).unwrap();
        let loaded_nippy = loaded_nippy.load_filters().unwrap();
        assert_eq!(nippy, loaded_nippy);

        if let Some(Compressors::Lz4(_)) = loaded_nippy.compressor() {
            let mut cursor = NippyJarCursor::new(&loaded_nippy).unwrap();

            // Iterate over compressed values and compare
            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!(
                    (row[0], row[1]),
                    (col1[row_index].as_slice(), col2[row_index].as_slice())
                );
                row_index += 1;
            }
        } else {
            panic!("Expected Lz4 compressor")
        }
    }

    #[test]
    fn test_zstd_no_dictionaries() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        let nippy = NippyJar::new_without_header(num_columns, file_path.path());
        assert!(nippy.compressor().is_none());

        let mut nippy =
            NippyJar::new_without_header(num_columns, file_path.path()).with_zstd(false, 5000);
        assert!(nippy.compressor().is_some());

        nippy.freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows).unwrap();

        let loaded_nippy =
            NippyJar::load_without_header(file_path.path()).unwrap().load_filters().unwrap();
        assert_eq!(nippy, loaded_nippy);

        if let Some(Compressors::Zstd(zstd)) = loaded_nippy.compressor() {
            assert!(!zstd.use_dict);

            let mut cursor = NippyJarCursor::new(&loaded_nippy).unwrap();

            // Iterate over compressed values and compare
            let mut row_index = 0usize;
            while let Some(row) = cursor.next_row().unwrap() {
                assert_eq!(
                    (row[0], row[1]),
                    (col1[row_index].as_slice(), col2[row_index].as_slice())
                );
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
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();
        let data = vec![col1.clone(), col2.clone()];

        let block_start = 500;

        #[derive(Serialize, Deserialize, Debug)]
        pub struct BlockJarHeader {
            block_start: usize,
        }

        // Create file
        {
            let mut nippy =
                NippyJar::new(num_columns, file_path.path(), BlockJarHeader { block_start })
                    .with_zstd(true, 5000)
                    .with_cuckoo_filter(col1.len())
                    .with_fmph();

            nippy.prepare_compression(data.clone()).unwrap();
            nippy.prepare_index(clone_with_result(&col1), col1.len()).unwrap();
            nippy
                .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
                .unwrap();
        }

        // Read file
        {
            let loaded_nippy =
                NippyJar::<BlockJarHeader>::load(file_path.path()).unwrap().load_filters().unwrap();

            assert!(loaded_nippy.compressor().is_some());
            assert!(loaded_nippy.filter.is_some());
            assert!(loaded_nippy.phf.is_some());
            assert_eq!(loaded_nippy.user_header().block_start, block_start);

            if let Some(Compressors::Zstd(_zstd)) = loaded_nippy.compressor() {
                let mut cursor = NippyJarCursor::new(&loaded_nippy).unwrap();

                // Iterate over compressed values and compare
                let mut row_num = 0usize;
                while let Some(row) = cursor.next_row().unwrap() {
                    assert_eq!(
                        (row[0], row[1]),
                        (data[0][row_num].as_slice(), data[1][row_num].as_slice())
                    );
                    row_num += 1;
                }

                // Shuffled for chaos.
                let mut data = col1.iter().zip(col2.iter()).enumerate().collect::<Vec<_>>();
                data.shuffle(&mut rand::thread_rng());

                for (row_num, (v0, v1)) in data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    {
                        let row_by_value = cursor
                            .row_by_key(v0)
                            .unwrap()
                            .unwrap()
                            .iter()
                            .map(|a| a.to_vec())
                            .collect::<Vec<_>>();
                        assert_eq!((&row_by_value[0], &row_by_value[1]), (v0, v1));

                        // Simulates `by_number` queries
                        let row_by_num = cursor.row_by_number(row_num).unwrap().unwrap();
                        assert_eq!(row_by_value, row_by_num);
                    }
                }
            }
        }
    }

    #[test]
    fn test_selectable_column_values() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();
        let data = vec![col1.clone(), col2.clone()];

        // Create file
        {
            let mut nippy = NippyJar::new_without_header(num_columns, file_path.path())
                .with_zstd(true, 5000)
                .with_cuckoo_filter(col1.len())
                .with_fmph();

            nippy.prepare_compression(data).unwrap();
            nippy.prepare_index(clone_with_result(&col1), col1.len()).unwrap();
            nippy
                .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
                .unwrap();
        }

        // Read file
        {
            let loaded_nippy =
                NippyJar::load_without_header(file_path.path()).unwrap().load_filters().unwrap();

            if let Some(Compressors::Zstd(_zstd)) = loaded_nippy.compressor() {
                let mut cursor = NippyJarCursor::new(&loaded_nippy).unwrap();

                // Shuffled for chaos.
                let mut data = col1.iter().zip(col2.iter()).enumerate().collect::<Vec<_>>();
                data.shuffle(&mut rand::thread_rng());

                // Imagine `Blocks` snapshot file has two columns: `Block | StoredWithdrawals`
                const BLOCKS_FULL_MASK: usize = 0b11;

                // Read both columns
                for (row_num, (v0, v1)) in &data {
                    // Simulates `by_hash` queries by iterating col1 values, which were used to
                    // create the inner index.
                    let row_by_value = cursor
                        .row_by_key_with_cols(v0, BLOCKS_FULL_MASK)
                        .unwrap()
                        .unwrap()
                        .iter()
                        .map(|a| a.to_vec())
                        .collect::<Vec<_>>();
                    assert_eq!((&row_by_value[0], &row_by_value[1]), (*v0, *v1));

                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_FULL_MASK)
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
                        .row_by_key_with_cols(v0, BLOCKS_BLOCK_MASK)
                        .unwrap()
                        .unwrap()
                        .iter()
                        .map(|a| a.to_vec())
                        .collect::<Vec<_>>();
                    assert_eq!(row_by_value.len(), 1);
                    assert_eq!(&row_by_value[0], *v0);

                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_BLOCK_MASK)
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
                        .row_by_key_with_cols(v0, BLOCKS_WITHDRAWAL_MASK)
                        .unwrap()
                        .unwrap()
                        .iter()
                        .map(|a| a.to_vec())
                        .collect::<Vec<_>>();
                    assert_eq!(row_by_value.len(), 1);
                    assert_eq!(&row_by_value[0], *v1);

                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_WITHDRAWAL_MASK)
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
                        .row_by_key_with_cols(v0, BLOCKS_EMPTY_MASK)
                        .unwrap()
                        .unwrap()
                        .is_empty());

                    // Simulates `by_number` queries
                    assert!(cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_EMPTY_MASK)
                        .unwrap()
                        .unwrap()
                        .is_empty());
                }
            }
        }
    }
}
