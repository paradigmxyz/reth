//! Database version utils.

use reth_fs_util::FsPathError;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// The name of the file that contains the version of the database.
pub const DB_VERSION_FILE_NAME: &str = "database.version";
/// The version of the database stored in the [`DB_VERSION_FILE_NAME`] file in the same directory as
/// database.
pub const DB_VERSION: u64 = 2;

/// Error when checking a database version using [`check_db_version_file`]
#[derive(thiserror::Error, Debug)]
pub enum DatabaseVersionError {
    /// Unable to determine the version of the database; the file is missing.
    #[error("unable to determine the version of the database, file is missing")]
    MissingFile,
    /// Unable to determine the version of the database; the file is malformed.
    #[error("unable to determine the version of the database, file is malformed")]
    MalformedFile,
    /// Breaking database change detected.
    ///
    /// Your database version is incompatible with the latest database version.
    #[error(
        "breaking database change detected: your database version (v{version}) \
         is incompatible with the latest database version (v{DB_VERSION})"
    )]
    VersionMismatch {
        /// The detected version in the database.
        version: u64,
    },
    /// IO error occurred while reading the database version file.
    #[error("IO error occurred while reading {path}: {err}")]
    IORead {
        /// The encountered IO error.
        err: io::Error,
        /// The path to the database version file.
        path: PathBuf,
    },
}

/// Checks the database version file with [`DB_VERSION_FILE_NAME`] name.
///
/// Returns [Ok] if file is found and has one line which equals to [`DB_VERSION`].
/// Otherwise, returns different [`DatabaseVersionError`] error variants.
pub fn check_db_version_file<P: AsRef<Path>>(db_path: P) -> Result<(), DatabaseVersionError> {
    let version = get_db_version(db_path)?;
    if version != DB_VERSION {
        return Err(DatabaseVersionError::VersionMismatch { version })
    }

    Ok(())
}

/// Returns the database version from file with [`DB_VERSION_FILE_NAME`] name.
///
/// Returns [Ok] if file is found and contains a valid version.
/// Otherwise, returns different [`DatabaseVersionError`] error variants.
pub fn get_db_version<P: AsRef<Path>>(db_path: P) -> Result<u64, DatabaseVersionError> {
    let version_file_path = db_version_file_path(db_path);
    match fs::read_to_string(&version_file_path) {
        Ok(raw_version) => {
            Ok(raw_version.parse::<u64>().map_err(|_| DatabaseVersionError::MalformedFile)?)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(DatabaseVersionError::MissingFile),
        Err(err) => Err(DatabaseVersionError::IORead { err, path: version_file_path }),
    }
}

/// Creates a database version file with [`DB_VERSION_FILE_NAME`] name containing [`DB_VERSION`]
/// string.
///
/// This function will create a file if it does not exist,
/// and will entirely replace its contents if it does.
///
/// The file is written atomically and synced, together with its directory, before this returns.
/// Otherwise a power loss shortly after the database is created can leave an empty file behind,
/// which [`check_db_version_file`] reports as [`DatabaseVersionError::MalformedFile`] on every
/// later start.
pub fn create_db_version_file<P: AsRef<Path>>(db_path: P) -> Result<(), FsPathError> {
    reth_fs_util::atomic_write_file(&db_version_file_path(db_path), |file| {
        file.write_all(DB_VERSION.to_string().as_bytes())
    })
}

/// Returns a database version file path.
pub fn db_version_file_path<P: AsRef<Path>>(db_path: P) -> PathBuf {
    db_path.as_ref().join(DB_VERSION_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::{
        check_db_version_file, create_db_version_file, db_version_file_path, DatabaseVersionError,
    };
    use assert_matches::assert_matches;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn created_file_passes_the_check() {
        let dir = tempdir().unwrap();
        create_db_version_file(&dir).unwrap();

        assert_matches!(check_db_version_file(&dir), Ok(()));
        // Only the version file is left behind, not the temporary file it was written through.
        let names: Vec<_> =
            fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, [super::DB_VERSION_FILE_NAME]);
    }

    #[test]
    fn created_file_replaces_a_malformed_one() {
        let dir = tempdir().unwrap();
        fs::write(db_version_file_path(&dir), "").unwrap();
        assert_matches!(check_db_version_file(&dir), Err(DatabaseVersionError::MalformedFile));

        create_db_version_file(&dir).unwrap();
        assert_matches!(check_db_version_file(&dir), Ok(()));
    }

    #[test]
    fn missing_file() {
        let dir = tempdir().unwrap();

        let result = check_db_version_file(&dir);
        assert_matches!(result, Err(DatabaseVersionError::MissingFile));
    }

    #[test]
    fn malformed_file() {
        let dir = tempdir().unwrap();
        fs::write(db_version_file_path(&dir), "invalid-version").unwrap();

        let result = check_db_version_file(&dir);
        assert_matches!(result, Err(DatabaseVersionError::MalformedFile));
    }

    #[test]
    fn version_mismatch() {
        let dir = tempdir().unwrap();
        fs::write(db_version_file_path(&dir), "0").unwrap();

        let result = check_db_version_file(&dir);
        assert_matches!(result, Err(DatabaseVersionError::VersionMismatch { version: 0 }));
    }
}
