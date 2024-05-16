//! Wrapper for `std::fs` methods

use std::{
    fs::{self, ReadDir},
    io,
    path::{Path, PathBuf},
};

/// Various error variants for `std::fs` operations that serve as an addition to the io::Error which
/// does not provide any information about the path.
#[derive(Debug, thiserror::Error)]
pub enum FsPathError {
    /// Error variant for failed write operation with additional path context.
    #[error("failed to write to {path:?}: {source}")]
    Write {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed read operation with additional path context.
    #[error("failed to read from {path:?}: {source}")]
    Read {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed read link operation with additional path context.
    #[error("failed to read from {path:?}: {source}")]
    ReadLink {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed file creation operation with additional path context.
    #[error("failed to create file {path:?}: {source}")]
    CreateFile {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed file removal operation with additional path context.
    #[error("failed to remove file {path:?}: {source}")]
    RemoveFile {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed directory creation operation with additional path context.
    #[error("failed to create dir {path:?}: {source}")]
    CreateDir {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed directory removal operation with additional path context.
    #[error("failed to remove dir {path:?}: {source}")]
    RemoveDir {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed directory read operation with additional path context.
    #[error("failed to read dir {path:?}: {source}")]
    ReadDir {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed file renaming operation with additional path context.
    #[error("failed to rename {from:?} to {to:?}: {source}")]
    Rename {
        /// The source `io::Error`.
        source: io::Error,
        /// The original path.
        from: PathBuf,
        /// The target path.
        to: PathBuf,
    },

    /// Error variant for failed file opening operation with additional path context.
    #[error("failed to open file {path:?}: {source}")]
    Open {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed file read as JSON operation with additional path context.
    #[error("failed to parse json file: {path:?}: {source}")]
    ReadJson {
        /// The source `serde_json::Error`.
        source: serde_json::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed JSON write to file operation with additional path context.
    #[error("failed to write to json file: {path:?}: {source}")]
    WriteJson {
        /// The source `serde_json::Error`.
        source: serde_json::Error,
        /// The path related to the operation.
        path: PathBuf,
    },

    /// Error variant for failed file metadata operation with additional path context.
    #[error("failed to get metadata for {path:?}: {source}")]
    Metadata {
        /// The source `io::Error`.
        source: io::Error,
        /// The path related to the operation.
        path: PathBuf,
    },
}

impl FsPathError {
    /// Returns the complementary error variant for [`std::fs::write`].
    pub fn write(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::Write { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::read`].
    pub fn read(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::Read { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::read_link`].
    pub fn read_link(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::ReadLink { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::File::create`].
    pub fn create_file(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::CreateFile { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::remove_file`].
    pub fn remove_file(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::RemoveFile { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::create_dir`].
    pub fn create_dir(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::CreateDir { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::remove_dir`].
    pub fn remove_dir(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::RemoveDir { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::read_dir`].
    pub fn read_dir(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::ReadDir { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::File::open`].
    pub fn open(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::Open { source, path: path.into() }
    }

    /// Returns the complementary error variant for [`std::fs::rename`].
    pub fn rename(source: io::Error, from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        FsPathError::Rename { source, from: from.into(), to: to.into() }
    }

    /// Returns the complementary error variant for [`std::fs::File::metadata`].
    pub fn metadata(source: io::Error, path: impl Into<PathBuf>) -> Self {
        FsPathError::Metadata { source, path: path.into() }
    }
}

type Result<T> = std::result::Result<T, FsPathError>;

/// Wrapper for `std::fs::read_to_string`
pub fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    fs::read_to_string(path).map_err(|err| FsPathError::read(err, path))
}

/// Read the entire contents of a file into a bytes vector.
///
/// Wrapper for `std::fs::read`
pub fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    fs::read(path).map_err(|err| FsPathError::read(err, path))
}

/// Wrapper for `std::fs::write`
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, contents).map_err(|err| FsPathError::write(err, path))
}

/// Wrapper for `std::fs::remove_dir_all`
pub fn remove_dir_all(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    fs::remove_dir_all(path).map_err(|err| FsPathError::remove_dir(err, path))
}

/// Wrapper for `std::fs::remove_file`
pub fn remove_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    fs::remove_file(path).map_err(|err| FsPathError::remove_file(err, path))
}

/// Wrapper for `std::fs::create_dir_all`
pub fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|err| FsPathError::create_dir(err, path))
}

/// Wrapper for `std::fs::read_dir`
pub fn read_dir(path: impl AsRef<Path>) -> Result<ReadDir> {
    let path = path.as_ref();
    fs::read_dir(path).map_err(|err| FsPathError::read_dir(err, path))
}

/// Wrapper for `std::fs::rename`
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    fs::rename(from, to).map_err(|err| FsPathError::rename(err, from, to))
}

/// Wrapper for `std::fs::metadata`
pub fn metadata(path: impl AsRef<Path>) -> Result<fs::Metadata> {
    let path = path.as_ref();
    fs::metadata(path).map_err(|err| FsPathError::metadata(err, path))
}
