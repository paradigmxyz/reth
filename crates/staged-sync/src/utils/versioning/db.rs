use crate::utils::versioning::client::CLIENT_VERSION;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The name of the file that contains the version of the database.
const DB_VERSION_FILE_NAME: &str = "database.version";
/// The version of the database stored in the [DB_VERSION_FILE_NAME] file in the same directory as
/// database. Example: `1`
const DB_VERSION: u64 = 1;

#[derive(thiserror::Error, Debug)]
pub(crate) enum DatabaseVersionError {
    #[error(
        "Reth (v{}) is unable to determine the version of the database, file is missing.",
        CLIENT_VERSION.to_string()
    )]
    MissingFile,
    #[error(
        "Reth (v{}) is unable to determine the version of the database, file is malformed.",
        CLIENT_VERSION.to_string()
    )]
    MalformedFile,
    #[error(
        "Reth (v{}) has detected a breaking database change. \
            Your database version (v{version}) is incompatible with the latest database version (v{}).",
        CLIENT_VERSION.to_string(),
        DB_VERSION.to_string()
    )]
    VersionMismatch { version: u64 },
    #[error(transparent)]
    IOError(#[from] io::Error),
}

pub(crate) fn check_db_version_file<P: AsRef<Path>>(
    db_path: P,
) -> Result<(), DatabaseVersionError> {
    match fs::read_to_string(db_version_file_path(db_path)) {
        Ok(raw_version) => {
            let version =
                raw_version.parse::<u64>().map_err(|_| DatabaseVersionError::MalformedFile)?;
            if version != DB_VERSION {
                return Err(DatabaseVersionError::VersionMismatch { version })
            }

            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(DatabaseVersionError::MissingFile),
        Err(err) => Err(err.into()),
    }
}

/// Creates a database version file with [DB_VERSION_FILE_NAME] name containing [DB_VERSION] string.
///
/// This function will create a file if it does not exist,
/// and will entirely replace its contents if it does.
pub(crate) fn create_db_version_file<P: AsRef<Path>>(db_path: P) -> io::Result<()> {
    fs::write(db_version_file_path(db_path), DB_VERSION.to_string())
}

fn db_version_file_path<P: AsRef<Path>>(db_path: P) -> PathBuf {
    db_path.as_ref().join(DB_VERSION_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use crate::utils::versioning::db::{
        check_db_version_file, db_version_file_path, DatabaseVersionError,
    };
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
