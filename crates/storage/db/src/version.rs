//! Database version utils.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The name of the file that contains the version of the database.
pub const DB_VERSION_FILE_NAME: &str = "database.version";
/// The version of the database stored in the [DB_VERSION_FILE_NAME] file in the same directory as
/// database. Example: `1`.
pub const DB_VERSION: u64 = 1;

/// Error when checking a database version using [check_db_version_file]
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum DatabaseVersionError {
    #[error("unable to determine the version of the database, file is missing")]
    MissingFile,
    #[error("unable to determine the version of the database, file is malformed")]
    MalformedFile,
    #[error(
        "breaking database change detected: your database version (v{version}) \
         is incompatible with the latest database version (v{DB_VERSION})"
    )]
    VersionMismatch { version: u64 },
    #[error("IO error occurred while reading {path}: {err}")]
    IORead { err: io::Error, path: PathBuf },
}

/// Checks the database version file with [DB_VERSION_FILE_NAME] name.
///
/// Returns [Ok] if file is found and has one line which equals to [DB_VERSION].
/// Otherwise, returns different [DatabaseVersionError] error variants.
pub fn check_db_version_file<P: AsRef<Path>>(db_path: P) -> Result<(), DatabaseVersionError> {
    let version = get_db_version(db_path)?;
    if version != DB_VERSION {
        return Err(DatabaseVersionError::VersionMismatch { version })
    }

    Ok(())
}

/// Returns the database version from file with [DB_VERSION_FILE_NAME] name.
///
/// Returns [Ok] if file is found and contains a valid version.
/// Otherwise, returns different [DatabaseVersionError] error variants.
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

/// Creates a database version file with [DB_VERSION_FILE_NAME] name containing [DB_VERSION] string.
///
/// This function will create a file if it does not exist,
/// and will entirely replace its contents if it does.
pub fn create_db_version_file<P: AsRef<Path>>(db_path: P) -> io::Result<()> {
    fs::write(db_version_file_path(db_path), DB_VERSION.to_string())
}

/// Returns a database version file path.
pub fn db_version_file_path<P: AsRef<Path>>(db_path: P) -> PathBuf {
    db_path.as_ref().join(DB_VERSION_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::{check_db_version_file, db_version_file_path, DatabaseVersionError};
    use assert_matches::assert_matches;
    use std::fs;
    use tempfile::tempdir;

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
