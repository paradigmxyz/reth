//! Era file format traits and I/O operations.

use crate::e2s::{error::E2sError, types::Version};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

/// Represents era file with generic content and identifier types
pub trait EraFileFormat: Sized {
    /// Content group type
    type EraGroup;

    /// The identifier type
    type Id: EraFileId;

    /// Get the version
    fn version(&self) -> &Version;

    /// Get the content group
    fn group(&self) -> &Self::EraGroup;

    /// Get the file identifier
    fn id(&self) -> &Self::Id;

    /// Create a new instance
    fn new(group: Self::EraGroup, id: Self::Id) -> Self;
}

/// Era file identifiers
pub trait EraFileId: Clone {
    /// File type for this identifier
    const FILE_TYPE: EraFileType;

    /// Number of items, slots for `era`, blocks for `era1`, per era
    const ITEMS_PER_ERA: u64;

    /// Get the network name
    fn network_name(&self) -> &str;

    /// Get the starting number (block or slot)
    fn start_number(&self) -> u64;

    /// Get the count of items
    fn count(&self) -> u32;

    /// Get the optional hash identifier
    fn hash(&self) -> Option<[u8; 4]>;

    /// Whether to include era count in filename
    fn include_era_count(&self) -> bool;

    /// Calculate era number
    fn era_number(&self) -> u64 {
        self.start_number() / Self::ITEMS_PER_ERA
    }

    /// Calculate the number of eras spanned per file.
    ///
    /// If the user can decide how many slots/blocks per era file there are, we need to calculate
    /// it. Most of the time it should be 1, but it can never be more than 2 eras per file
    /// as there is a maximum of 8192 slots/blocks per era file.
    fn era_count(&self) -> u64 {
        if self.count() == 0 {
            return 0;
        }
        let first_era = self.era_number();
        let last_number = self.start_number() + self.count() as u64 - 1;
        let last_era = last_number / Self::ITEMS_PER_ERA;
        last_era - first_era + 1
    }

    /// Convert to standardized file name.
    fn to_file_name(&self) -> String {
        Self::FILE_TYPE.format_filename(
            self.network_name(),
            self.era_number(),
            self.hash(),
            self.include_era_count(),
            self.era_count(),
        )
    }
}

/// [`StreamReader`] for reading era-format files
pub trait StreamReader<R: Read + Seek>: Sized {
    /// The file type the reader produces
    type File: EraFileFormat;

    /// The iterator type for streaming data
    type Iterator;

    /// Create a new reader
    fn new(reader: R) -> Self;

    /// Read and parse the complete file
    fn read(self, network_name: String) -> Result<Self::File, E2sError>;

    /// Get an iterator for streaming processing
    fn iter(self) -> Self::Iterator;
}

/// [`FileReader`] provides reading era file operations for era files
pub trait FileReader: StreamReader<File> {
    /// Opens and reads an era file from the given path
    fn open<P: AsRef<Path>>(
        path: P,
        network_name: impl Into<String>,
    ) -> Result<Self::File, E2sError> {
        let file = File::open(path).map_err(E2sError::Io)?;
        let reader = Self::new(file);
        reader.read(network_name.into())
    }
}

/// [`StreamWriter`] for writing era-format files
pub trait StreamWriter<W: Write>: Sized {
    /// The file type this writer handles
    type File: EraFileFormat;

    /// Create a new writer
    fn new(writer: W) -> Self;

    /// Writer version
    fn write_version(&mut self) -> Result<(), E2sError>;

    /// Write a complete era file
    fn write_file(&mut self, file: &Self::File) -> Result<(), E2sError>;

    /// Flush any buffered data
    fn flush(&mut self) -> Result<(), E2sError>;
}

/// [`StreamWriter`] provides writing file operations for era files
pub trait FileWriter {
    /// Era file type the writer handles
    type File: EraFileFormat<Id: EraFileId>;

    /// Creates a new file at the specified path and writes the era file to it
    fn create<P: AsRef<Path>>(path: P, file: &Self::File) -> Result<(), E2sError>;

    /// Creates a file in the directory using standardized era naming
    fn create_with_id<P: AsRef<Path>>(directory: P, file: &Self::File) -> Result<(), E2sError>;
}

impl<T: StreamWriter<File>> FileWriter for T {
    type File = T::File;

    /// Creates a new file at the specified path and writes the era file to it
    fn create<P: AsRef<Path>>(path: P, file: &Self::File) -> Result<(), E2sError> {
        let file_handle = File::create(path).map_err(E2sError::Io)?;
        let mut writer = Self::new(file_handle);
        writer.write_file(file)?;
        Ok(())
    }

    /// Creates a file in the directory using standardized era naming
    fn create_with_id<P: AsRef<Path>>(directory: P, file: &Self::File) -> Result<(), E2sError> {
        let filename = file.id().to_file_name();
        let path = directory.as_ref().join(filename);
        Self::create(path, file)
    }
}

/// Era file type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EraFileType {
    /// Consensus layer ERA file, `.era`
    /// Contains beacon blocks and states
    Era,
    /// Execution layer ERA1 file, `.era1`
    /// Contains execution blocks pre-merge
    Era1,
}

impl EraFileType {
    /// Get the file extension for this type, dot included
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Era => ".era",
            Self::Era1 => ".era1",
        }
    }

    /// Detect file type from a filename
    pub fn from_filename(filename: &str) -> Option<Self> {
        if filename.ends_with(".era") {
            Some(Self::Era)
        } else if filename.ends_with(".era1") {
            Some(Self::Era1)
        } else {
            None
        }
    }

    /// Generate era file name.
    ///
    /// Standard format: `<config-name>-<era-number>-<short-historical-root>.<ext>`
    /// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md#file-name>
    ///
    /// With era count (for custom exports):
    /// `<config-name>-<era-number>-<era-count>-<short-historical-root>.<ext>`
    pub fn format_filename(
        &self,
        network_name: &str,
        era_number: u64,
        hash: Option<[u8; 4]>,
        include_era_count: bool,
        era_count: u64,
    ) -> String {
        let hash = format_hash(hash);

        if include_era_count {
            format!(
                "{}-{:05}-{:05}-{}{}",
                network_name,
                era_number,
                era_count,
                hash,
                self.extension()
            )
        } else {
            format!("{}-{:05}-{}{}", network_name, era_number, hash, self.extension())
        }
    }

    /// Detect file type from URL
    /// By default, it assumes `Era` type
    pub fn from_url(url: &str) -> Self {
        if url.contains("era1") {
            Self::Era1
        } else {
            Self::Era
        }
    }
}

/// Format hash as hex string, or placeholder if none
pub fn format_hash(hash: Option<[u8; 4]>) -> String {
    match hash {
        Some(h) => format!("{:02x}{:02x}{:02x}{:02x}", h[0], h[1], h[2], h[3]),
        None => "00000000".to_string(),
    }
}
