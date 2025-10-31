//! Represents reading and writing operations' era file

use crate::{e2s_types::Version, E2sError};
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
    /// Convert to standardized file name
    fn to_file_name(&self) -> String;

    /// Get the network name
    fn network_name(&self) -> &str;

    /// Get the starting number (block or slot)
    fn start_number(&self) -> u64;

    /// Get the count of items
    fn count(&self) -> u32;
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
