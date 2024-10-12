//! Immutable data store format.
//!
//! *Warning*: The `NippyJar` encoding format and its implementations are
//! designed for storing and retrieving data internally. They are not hardened
//! to safely read potentially malicious data.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::{
    error::Error as StdError,
    fs::File,
    ops::Range,
    path::{Path, PathBuf},
};

// Windows specific extension for std::fs
#[cfg(windows)]
use std::os::windows::prelude::OpenOptionsExt;

use tracing::*;

pub mod compression;
#[cfg(test)]
use compression::Compression;
use compression::Compressors;

/// empty enum for backwards compatibility
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum Functions {}

/// empty enum for backwards compatibility
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum InclusionFilters {}

mod error;
pub use error::NippyJarError;

mod cursor;
pub use cursor::NippyJarCursor;

mod writer;
pub use writer::NippyJarWriter;

mod consistency;
pub use consistency::NippyJarChecker;

const NIPPY_JAR_VERSION: usize = 1;

const INDEX_FILE_EXTENSION: &str = "idx";
const OFFSETS_FILE_EXTENSION: &str = "off";
pub const CONFIG_FILE_EXTENSION: &str = "conf";

/// A [`RefRow`] is a list of column value slices pointing to either an internal buffer or a
/// memory-mapped file.
type RefRow<'a> = Vec<&'a [u8]>;

/// Alias type for a column value wrapped in `Result`.
pub type ColumnResult<T> = Result<T, Box<dyn StdError + Send + Sync>>;

/// A trait for the user-defined header of [`NippyJar`].
pub trait NippyJarHeader:
    Send + Sync + Serialize + for<'b> Deserialize<'b> + std::fmt::Debug + 'static
{
}

// Blanket implementation for all types that implement the required traits.
impl<T> NippyJarHeader for T where
    T: Send + Sync + Serialize + for<'b> Deserialize<'b> + std::fmt::Debug + 'static
{
}

/// `NippyJar` is a specialized storage format designed for immutable data.
///
/// Data is organized into a columnar format, enabling column-based compression. Data retrieval
/// entails consulting an offset list and fetching the data from file via `mmap`.
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct NippyJar<H = ()> {
    /// The version of the `NippyJar` format.
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
    /// Optional field for backwards compatibility
    filter: Option<InclusionFilters>,
    #[serde(skip)]
    /// Optional field for backwards compatibility
    phf: Option<Functions>,
    /// Maximum uncompressed row size of the set. This will enable decompression without any
    /// resizing of the output buffer.
    max_row_size: usize,
    /// Data path for file. Supporting files will have a format `{path}.{extension}`.
    #[serde(skip)]
    path: PathBuf,
}

impl<H: NippyJarHeader> std::fmt::Debug for NippyJar<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NippyJar")
            .field("version", &self.version)
            .field("user_header", &self.user_header)
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("compressor", &self.compressor)
            .field("filter", &self.filter)
            .field("phf", &self.phf)
            .field("path", &self.path)
            .field("max_row_size", &self.max_row_size)
            .finish_non_exhaustive()
    }
}

impl NippyJar<()> {
    /// Creates a new [`NippyJar`] without an user-defined header data.
    pub fn new_without_header(columns: usize, path: &Path) -> Self {
        Self::new(columns, path, ())
    }

    /// Loads the file configuration and returns [`Self`] on a jar without user-defined header data.
    pub fn load_without_header(path: &Path) -> Result<Self, NippyJarError> {
        Self::load(path)
    }
}

impl<H: NippyJarHeader> NippyJar<H> {
    /// Creates a new [`NippyJar`] with a user-defined header data.
    pub fn new(columns: usize, path: &Path, user_header: H) -> Self {
        Self {
            version: NIPPY_JAR_VERSION,
            user_header,
            columns,
            rows: 0,
            max_row_size: 0,
            compressor: None,
            filter: None,
            phf: None,
            path: path.to_path_buf(),
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

    /// Gets a reference to the user header.
    pub const fn user_header(&self) -> &H {
        &self.user_header
    }

    /// Gets total columns in jar.
    pub const fn columns(&self) -> usize {
        self.columns
    }

    /// Gets total rows in jar.
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Gets a reference to the compressor.
    pub const fn compressor(&self) -> Option<&Compressors> {
        self.compressor.as_ref()
    }

    /// Gets a mutable reference to the compressor.
    pub fn compressor_mut(&mut self) -> Option<&mut Compressors> {
        self.compressor.as_mut()
    }

    /// Loads the file configuration and returns [`Self`].
    ///
    /// **The user must ensure the header type matches the one used during the jar's creation.**
    pub fn load(path: &Path) -> Result<Self, NippyJarError> {
        // Read [`Self`] located at the data file.
        let config_path = path.with_extension(CONFIG_FILE_EXTENSION);
        let config_file = File::open(&config_path)
            .map_err(|err| reth_fs_util::FsPathError::open(err, config_path))?;

        let mut obj: Self = bincode::deserialize_from(&config_file)?;
        obj.path = path.to_path_buf();
        Ok(obj)
    }

    /// Returns the path for the data file
    pub fn data_path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Returns the path for the index file
    pub fn index_path(&self) -> PathBuf {
        self.path.with_extension(INDEX_FILE_EXTENSION)
    }

    /// Returns the path for the offsets file
    pub fn offsets_path(&self) -> PathBuf {
        self.path.with_extension(OFFSETS_FILE_EXTENSION)
    }

    /// Returns the path for the config file
    pub fn config_path(&self) -> PathBuf {
        self.path.with_extension(CONFIG_FILE_EXTENSION)
    }

    /// Deletes from disk this [`NippyJar`] alongside every satellite file.
    pub fn delete(self) -> Result<(), NippyJarError> {
        // TODO(joshie): ensure consistency on unexpected shutdown

        for path in
            [self.data_path().into(), self.index_path(), self.offsets_path(), self.config_path()]
        {
            if path.exists() {
                reth_fs_util::remove_file(path)?;
            }
        }

        Ok(())
    }

    /// Returns a [`DataReader`] of the data and offset file
    pub fn open_data_reader(&self) -> Result<DataReader, NippyJarError> {
        DataReader::new(self.data_path())
    }

    /// Writes all necessary configuration to file.
    fn freeze_config(&self) -> Result<(), NippyJarError> {
        Ok(reth_fs_util::atomic_write_file(&self.config_path(), |file| {
            bincode::serialize_into(file, &self)
        })?)
    }
}

#[cfg(test)]
impl<H: NippyJarHeader> NippyJar<H> {
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

    /// Writes all data and configuration to a file and the offset index to another.
    pub fn freeze(
        self,
        columns: Vec<impl IntoIterator<Item = ColumnResult<Vec<u8>>>>,
        total_rows: u64,
    ) -> Result<Self, NippyJarError> {
        self.check_before_freeze(&columns)?;

        debug!(target: "nippy-jar", path=?self.data_path(), "Opening data file.");

        // Creates the writer, data and offsets file
        let mut writer = NippyJarWriter::new(self)?;

        // Append rows to file while holding offsets in memory
        writer.append_rows(columns, total_rows)?;

        // Flushes configuration and offsets to disk
        writer.commit()?;

        debug!(target: "nippy-jar", ?writer, "Finished writing data.");

        Ok(writer.into_jar())
    }

    /// Safety checks before creating and returning a [`File`] handle to write data to.
    fn check_before_freeze(
        &self,
        columns: &[impl IntoIterator<Item = ColumnResult<Vec<u8>>>],
    ) -> Result<(), NippyJarError> {
        if columns.len() != self.columns {
            return Err(NippyJarError::ColumnLenMismatch(self.columns, columns.len()))
        }

        if let Some(compression) = &self.compressor {
            if !compression.is_ready() {
                return Err(NippyJarError::CompressorNotReady)
            }
        }

        Ok(())
    }
}

/// Manages the reading of static file data using memory-mapped files.
///
/// Holds file and mmap descriptors of the data and offsets files of a `static_file`.
#[derive(Debug)]
pub struct DataReader {
    /// Data file descriptor. Needs to be kept alive as long as `data_mmap` handle.
    #[allow(dead_code)]
    data_file: File,
    /// Mmap handle for data.
    data_mmap: Mmap,
    /// Offset file descriptor. Needs to be kept alive as long as `offset_mmap` handle.
    #[allow(dead_code)]
    offset_file: File,
    /// Mmap handle for offsets.
    offset_mmap: Mmap,
    /// Number of bytes that represent one offset.
    offset_size: u8,
}

impl DataReader {
    /// Reads the respective data and offsets file and returns [`DataReader`].
    pub fn new(path: impl AsRef<Path>) -> Result<Self, NippyJarError> {
        let data_file = File::open(path.as_ref())?;
        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let data_mmap = unsafe { Mmap::map(&data_file)? };

        let offset_file = File::open(path.as_ref().with_extension(OFFSETS_FILE_EXTENSION))?;
        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let offset_mmap = unsafe { Mmap::map(&offset_file)? };

        // First byte is the size of one offset in bytes
        let offset_size = offset_mmap[0];

        // Ensure that the size of an offset is at most 8 bytes.
        if offset_size > 8 {
            return Err(NippyJarError::OffsetSizeTooBig { offset_size })
        } else if offset_size == 0 {
            return Err(NippyJarError::OffsetSizeTooSmall { offset_size })
        }

        Ok(Self { data_file, data_mmap, offset_file, offset_size, offset_mmap })
    }

    /// Returns the offset for the requested data index
    pub fn offset(&self, index: usize) -> Result<u64, NippyJarError> {
        // + 1 represents the offset_len u8 which is in the beginning of the file
        let from = index * self.offset_size as usize + 1;

        self.offset_at(from)
    }

    /// Returns the offset for the requested data index starting from the end
    pub fn reverse_offset(&self, index: usize) -> Result<u64, NippyJarError> {
        let offsets_file_size = self.offset_file.metadata()?.len() as usize;

        if offsets_file_size > 1 {
            let from = offsets_file_size - self.offset_size as usize * (index + 1);

            self.offset_at(from)
        } else {
            Ok(0)
        }
    }

    /// Returns total number of offsets in the file.
    /// The size of one offset is determined by the file itself.
    pub fn offsets_count(&self) -> Result<usize, NippyJarError> {
        Ok((self.offset_file.metadata()?.len().saturating_sub(1) / self.offset_size as u64)
            as usize)
    }

    /// Reads one offset-sized (determined by the offset file) u64 at the provided index.
    fn offset_at(&self, index: usize) -> Result<u64, NippyJarError> {
        let mut buffer: [u8; 8] = [0; 8];

        let offset_end = index.saturating_add(self.offset_size as usize);
        if offset_end > self.offset_mmap.len() {
            return Err(NippyJarError::OffsetOutOfBounds { index })
        }

        buffer[..self.offset_size as usize].copy_from_slice(&self.offset_mmap[index..offset_end]);
        Ok(u64::from_le_bytes(buffer))
    }

    /// Returns number of bytes that represent one offset.
    pub const fn offset_size(&self) -> u8 {
        self.offset_size
    }

    /// Returns the underlying data as a slice of bytes for the provided range.
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
    use compression::Compression;
    use rand::{rngs::SmallRng, seq::SliceRandom, RngCore, SeedableRng};
    use std::{fs::OpenOptions, io::Read};

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
    fn test_config_serialization() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let jar = NippyJar::new_without_header(23, file.path()).with_lz4();
        jar.freeze_config().unwrap();

        let mut config_file = OpenOptions::new().read(true).open(jar.config_path()).unwrap();
        let config_file_len = config_file.metadata().unwrap().len();
        assert_eq!(config_file_len, 37);

        let mut buf = Vec::with_capacity(config_file_len as usize);
        config_file.read_to_end(&mut buf).unwrap();

        assert_eq!(
            vec![
                1, 0, 0, 0, 0, 0, 0, 0, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ],
            buf
        );

        let mut read_jar = bincode::deserialize_from::<_, NippyJar>(&buf[..]).unwrap();
        // Path is not ser/de
        read_jar.path = file.path().to_path_buf();
        assert_eq!(jar, read_jar);
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

        let mut nippy =
            NippyJar::new_without_header(num_columns, file_path.path()).with_zstd(true, 5000);
        assert!(nippy.compressor().is_some());

        nippy.prepare_compression(vec![col1.clone(), col2.clone()]).unwrap();

        if let Some(Compressors::Zstd(zstd)) = &nippy.compressor() {
            assert!(matches!(
                (&zstd.state, zstd.dictionaries.as_ref().map(|dict| dict.len())),
                (compression::ZstdState::Ready, Some(columns)) if columns == num_columns
            ));
        }

        let nippy = nippy
            .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
            .unwrap();

        let loaded_nippy = NippyJar::load_without_header(file_path.path()).unwrap();
        assert_eq!(nippy.version, loaded_nippy.version);
        assert_eq!(nippy.columns, loaded_nippy.columns);
        assert_eq!(nippy.filter, loaded_nippy.filter);
        assert_eq!(nippy.phf, loaded_nippy.phf);
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

        let nippy = NippyJar::new_without_header(num_columns, file_path.path()).with_lz4();
        assert!(nippy.compressor().is_some());

        let nippy = nippy
            .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
            .unwrap();

        let loaded_nippy = NippyJar::load_without_header(file_path.path()).unwrap();
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

        let nippy =
            NippyJar::new_without_header(num_columns, file_path.path()).with_zstd(false, 5000);
        assert!(nippy.compressor().is_some());

        let nippy = nippy
            .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
            .unwrap();

        let loaded_nippy = NippyJar::load_without_header(file_path.path()).unwrap();
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

    /// Tests `NippyJar` with everything enabled.
    #[test]
    fn test_full_nippy_jar() {
        let (col1, col2) = test_data(None);
        let num_rows = col1.len() as u64;
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();
        let data = vec![col1.clone(), col2.clone()];

        let block_start = 500;

        #[derive(Serialize, Deserialize, Debug)]
        struct BlockJarHeader {
            block_start: usize,
        }

        // Create file
        {
            let mut nippy =
                NippyJar::new(num_columns, file_path.path(), BlockJarHeader { block_start })
                    .with_zstd(true, 5000);

            nippy.prepare_compression(data.clone()).unwrap();
            nippy
                .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
                .unwrap();
        }

        // Read file
        {
            let loaded_nippy = NippyJar::<BlockJarHeader>::load(file_path.path()).unwrap();

            assert!(loaded_nippy.compressor().is_some());
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
                    // Simulates `by_number` queries
                    let row_by_num = cursor.row_by_number(row_num).unwrap().unwrap();
                    assert_eq!((&row_by_num[0].to_vec(), &row_by_num[1].to_vec()), (v0, v1));
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
            let mut nippy =
                NippyJar::new_without_header(num_columns, file_path.path()).with_zstd(true, 5000);
            nippy.prepare_compression(data).unwrap();
            nippy
                .freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], num_rows)
                .unwrap();
        }

        // Read file
        {
            let loaded_nippy = NippyJar::load_without_header(file_path.path()).unwrap();

            if let Some(Compressors::Zstd(_zstd)) = loaded_nippy.compressor() {
                let mut cursor = NippyJarCursor::new(&loaded_nippy).unwrap();

                // Shuffled for chaos.
                let mut data = col1.iter().zip(col2.iter()).enumerate().collect::<Vec<_>>();
                data.shuffle(&mut rand::thread_rng());

                // Imagine `Blocks` static file has two columns: `Block | StoredWithdrawals`
                const BLOCKS_FULL_MASK: usize = 0b11;

                // Read both columns
                for (row_num, (v0, v1)) in &data {
                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_FULL_MASK)
                        .unwrap()
                        .unwrap();
                    assert_eq!((&row_by_num[0].to_vec(), &row_by_num[1].to_vec()), (*v0, *v1));
                }

                // Read first column only: `Block`
                const BLOCKS_BLOCK_MASK: usize = 0b01;
                for (row_num, (v0, _)) in &data {
                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_BLOCK_MASK)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_num.len(), 1);
                    assert_eq!(&row_by_num[0].to_vec(), *v0);
                }

                // Read second column only: `Block`
                const BLOCKS_WITHDRAWAL_MASK: usize = 0b10;
                for (row_num, (_, v1)) in &data {
                    // Simulates `by_number` queries
                    let row_by_num = cursor
                        .row_by_number_with_cols(*row_num, BLOCKS_WITHDRAWAL_MASK)
                        .unwrap()
                        .unwrap();
                    assert_eq!(row_by_num.len(), 1);
                    assert_eq!(&row_by_num[0].to_vec(), *v1);
                }

                // Read nothing
                const BLOCKS_EMPTY_MASK: usize = 0b00;
                for (row_num, _) in &data {
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

    #[test]
    fn test_writer() {
        let (col1, col2) = test_data(None);
        let num_columns = 2;
        let file_path = tempfile::NamedTempFile::new().unwrap();

        append_two_rows(num_columns, file_path.path(), &col1, &col2);

        // Appends a third row and prunes two rows, to make sure we prune from memory and disk
        // offset list
        prune_rows(num_columns, file_path.path(), &col1, &col2);

        // Should be able to append new rows
        append_two_rows(num_columns, file_path.path(), &col1, &col2);

        // Simulate an unexpected shutdown before there's a chance to commit, and see that it
        // unwinds successfully
        test_append_consistency_no_commit(file_path.path(), &col1, &col2);

        // Simulate an unexpected shutdown during commit, and see that it unwinds successfully
        test_append_consistency_partial_commit(file_path.path(), &col1, &col2);
    }

    #[test]
    fn test_pruner() {
        let (col1, col2) = test_data(None);
        let num_columns = 2;
        let num_rows = 2;

        // (missing_offsets, expected number of rows)
        // If a row wasn't fully pruned, then it should clear it up as well
        let missing_offsets_scenarios = [(1, 1), (2, 1), (3, 0)];

        for (missing_offsets, expected_rows) in missing_offsets_scenarios {
            let file_path = tempfile::NamedTempFile::new().unwrap();

            append_two_rows(num_columns, file_path.path(), &col1, &col2);

            simulate_interrupted_prune(num_columns, file_path.path(), num_rows, missing_offsets);

            let nippy = NippyJar::load_without_header(file_path.path()).unwrap();
            assert_eq!(nippy.rows, expected_rows);
        }
    }

    fn test_append_consistency_partial_commit(
        file_path: &Path,
        col1: &[Vec<u8>],
        col2: &[Vec<u8>],
    ) {
        let nippy = NippyJar::load_without_header(file_path).unwrap();

        // Set the baseline that should be unwinded to
        let initial_rows = nippy.rows;
        let initial_data_size =
            File::open(nippy.data_path()).unwrap().metadata().unwrap().len() as usize;
        let initial_offset_size =
            File::open(nippy.offsets_path()).unwrap().metadata().unwrap().len() as usize;
        assert!(initial_data_size > 0);
        assert!(initial_offset_size > 0);

        // Appends a third row
        let mut writer = NippyJarWriter::new(nippy).unwrap();
        writer.append_column(Some(Ok(&col1[2]))).unwrap();
        writer.append_column(Some(Ok(&col2[2]))).unwrap();

        // Makes sure it doesn't write the last one offset (which is the expected file data size)
        let _ = writer.offsets_mut().pop();

        // `commit_offsets` is not a pub function. we call it here to simulate the shutdown before
        // it can flush nippy.rows (config) to disk.
        writer.commit_offsets().unwrap();

        // Simulate an unexpected shutdown of the writer, before it can finish commit()
        drop(writer);

        let nippy = NippyJar::load_without_header(file_path).unwrap();
        assert_eq!(initial_rows, nippy.rows);

        // Data was written successfully
        let new_data_size =
            File::open(nippy.data_path()).unwrap().metadata().unwrap().len() as usize;
        assert_eq!(new_data_size, initial_data_size + col1[2].len() + col2[2].len());

        // It should be + 16 (two columns were added), but there's a missing one (the one we pop)
        assert_eq!(
            initial_offset_size + 8,
            File::open(nippy.offsets_path()).unwrap().metadata().unwrap().len() as usize
        );

        // Writer will execute a consistency check and verify first that the offset list on disk
        // doesn't match the nippy.rows, and prune it. Then, it will prune the data file
        // accordingly as well.
        let writer = NippyJarWriter::new(nippy).unwrap();
        assert_eq!(initial_rows, writer.rows());
        assert_eq!(
            initial_offset_size,
            File::open(writer.offsets_path()).unwrap().metadata().unwrap().len() as usize
        );
        assert_eq!(
            initial_data_size,
            File::open(writer.data_path()).unwrap().metadata().unwrap().len() as usize
        );
    }

    fn test_append_consistency_no_commit(file_path: &Path, col1: &[Vec<u8>], col2: &[Vec<u8>]) {
        let nippy = NippyJar::load_without_header(file_path).unwrap();

        // Set the baseline that should be unwinded to
        let initial_rows = nippy.rows;
        let initial_data_size =
            File::open(nippy.data_path()).unwrap().metadata().unwrap().len() as usize;
        let initial_offset_size =
            File::open(nippy.offsets_path()).unwrap().metadata().unwrap().len() as usize;
        assert!(initial_data_size > 0);
        assert!(initial_offset_size > 0);

        // Appends a third row, so we have an offset list in memory, which is not flushed to disk,
        // while the data has been.
        let mut writer = NippyJarWriter::new(nippy).unwrap();
        writer.append_column(Some(Ok(&col1[2]))).unwrap();
        writer.append_column(Some(Ok(&col2[2]))).unwrap();

        // Simulate an unexpected shutdown of the writer, before it can call commit()
        drop(writer);

        let nippy = NippyJar::load_without_header(file_path).unwrap();
        assert_eq!(initial_rows, nippy.rows);

        // Data was written successfully
        let new_data_size =
            File::open(nippy.data_path()).unwrap().metadata().unwrap().len() as usize;
        assert_eq!(new_data_size, initial_data_size + col1[2].len() + col2[2].len());

        // Since offsets only get written on commit(), this remains the same
        assert_eq!(
            initial_offset_size,
            File::open(nippy.offsets_path()).unwrap().metadata().unwrap().len() as usize
        );

        // Writer will execute a consistency check and verify that the data file has more data than
        // it should, and resets it to the last offset of the list (on disk here)
        let writer = NippyJarWriter::new(nippy).unwrap();
        assert_eq!(initial_rows, writer.rows());
        assert_eq!(
            initial_data_size,
            File::open(writer.data_path()).unwrap().metadata().unwrap().len() as usize
        );
    }

    fn append_two_rows(num_columns: usize, file_path: &Path, col1: &[Vec<u8>], col2: &[Vec<u8>]) {
        // Create and add 1 row
        {
            let nippy = NippyJar::new_without_header(num_columns, file_path);
            nippy.freeze_config().unwrap();
            assert_eq!(nippy.max_row_size, 0);
            assert_eq!(nippy.rows, 0);

            let mut writer = NippyJarWriter::new(nippy).unwrap();
            assert_eq!(writer.column(), 0);

            writer.append_column(Some(Ok(&col1[0]))).unwrap();
            assert_eq!(writer.column(), 1);
            assert!(writer.is_dirty());

            writer.append_column(Some(Ok(&col2[0]))).unwrap();
            assert!(writer.is_dirty());

            // Adding last column of a row resets writer and updates jar config
            assert_eq!(writer.column(), 0);

            // One offset per column + 1 offset at the end representing the expected file data size
            assert_eq!(writer.offsets().len(), 3);
            let expected_data_file_size = *writer.offsets().last().unwrap();
            writer.commit().unwrap();
            assert!(!writer.is_dirty());

            assert_eq!(writer.max_row_size(), col1[0].len() + col2[0].len());
            assert_eq!(writer.rows(), 1);
            assert_eq!(
                File::open(writer.offsets_path()).unwrap().metadata().unwrap().len(),
                1 + num_columns as u64 * 8 + 8
            );
            assert_eq!(
                File::open(writer.data_path()).unwrap().metadata().unwrap().len(),
                expected_data_file_size
            );
        }

        // Load and add 1 row
        {
            let nippy = NippyJar::load_without_header(file_path).unwrap();
            // Check if it was committed successfully
            assert_eq!(nippy.max_row_size, col1[0].len() + col2[0].len());
            assert_eq!(nippy.rows, 1);

            let mut writer = NippyJarWriter::new(nippy).unwrap();
            assert_eq!(writer.column(), 0);

            writer.append_column(Some(Ok(&col1[1]))).unwrap();
            assert_eq!(writer.column(), 1);

            writer.append_column(Some(Ok(&col2[1]))).unwrap();

            // Adding last column of a row resets writer and updates jar config
            assert_eq!(writer.column(), 0);

            // One offset per column + 1 offset at the end representing the expected file data size
            assert_eq!(writer.offsets().len(), 3);
            let expected_data_file_size = *writer.offsets().last().unwrap();
            writer.commit().unwrap();

            assert_eq!(writer.max_row_size(), col1[0].len() + col2[0].len());
            assert_eq!(writer.rows(), 2);
            assert_eq!(
                File::open(writer.offsets_path()).unwrap().metadata().unwrap().len(),
                1 + writer.rows() as u64 * num_columns as u64 * 8 + 8
            );
            assert_eq!(
                File::open(writer.data_path()).unwrap().metadata().unwrap().len(),
                expected_data_file_size
            );
        }
    }

    fn prune_rows(num_columns: usize, file_path: &Path, col1: &[Vec<u8>], col2: &[Vec<u8>]) {
        let nippy = NippyJar::load_without_header(file_path).unwrap();
        let mut writer = NippyJarWriter::new(nippy).unwrap();

        // Appends a third row, so we have an offset list in memory, which is not flushed to disk
        writer.append_column(Some(Ok(&col1[2]))).unwrap();
        writer.append_column(Some(Ok(&col2[2]))).unwrap();
        assert!(writer.is_dirty());

        // This should prune from the on-memory offset list and ondisk offset list
        writer.prune_rows(2).unwrap();
        assert_eq!(writer.rows(), 1);

        assert_eq!(
            File::open(writer.offsets_path()).unwrap().metadata().unwrap().len(),
            1 + writer.rows() as u64 * num_columns as u64 * 8 + 8
        );

        let expected_data_size = col1[0].len() + col2[0].len();
        assert_eq!(
            File::open(writer.data_path()).unwrap().metadata().unwrap().len() as usize,
            expected_data_size
        );

        let nippy = NippyJar::load_without_header(file_path).unwrap();
        {
            let data_reader = nippy.open_data_reader().unwrap();
            // there are only two valid offsets. so index 2 actually represents the expected file
            // data size.
            assert_eq!(data_reader.offset(2).unwrap(), expected_data_size as u64);
        }

        // This should prune from the ondisk offset list and clear the jar.
        let mut writer = NippyJarWriter::new(nippy).unwrap();
        writer.prune_rows(1).unwrap();
        assert!(writer.is_dirty());

        assert_eq!(writer.rows(), 0);
        assert_eq!(writer.max_row_size(), 0);
        assert_eq!(File::open(writer.data_path()).unwrap().metadata().unwrap().len() as usize, 0);
        // Only the byte that indicates how many bytes per offset should be left
        assert_eq!(
            File::open(writer.offsets_path()).unwrap().metadata().unwrap().len() as usize,
            1
        );
        writer.commit().unwrap();
        assert!(!writer.is_dirty());
    }

    fn simulate_interrupted_prune(
        num_columns: usize,
        file_path: &Path,
        num_rows: u64,
        missing_offsets: u64,
    ) {
        let nippy = NippyJar::load_without_header(file_path).unwrap();
        let reader = nippy.open_data_reader().unwrap();
        let offsets_file =
            OpenOptions::new().read(true).write(true).open(nippy.offsets_path()).unwrap();
        let offsets_len = 1 + num_rows * num_columns as u64 * 8 + 8;
        assert_eq!(offsets_len, offsets_file.metadata().unwrap().len());

        let data_file = OpenOptions::new().read(true).write(true).open(nippy.data_path()).unwrap();
        let data_len = reader.reverse_offset(0).unwrap();
        assert_eq!(data_len, data_file.metadata().unwrap().len());

        // each data column is 32 bytes long
        // by deleting from the data file, the `consistency_check` will go through both branches:
        //      when the offset list wasn't updated after clearing the data (data_len > last
        // offset).      fixing above, will lead to offset count not match the rows (*
        // columns) of the configuration file
        data_file.set_len(data_len - 32 * missing_offsets).unwrap();

        // runs the consistency check.
        let _ = NippyJarWriter::new(nippy).unwrap();
    }
}
