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
enum DatabaseVersionError {
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
        "Reth (v{}) has detected a breaking database change.\
            Your database version (v{version}) is incompatible with the latest database version (v{}).",
        CLIENT_VERSION.to_string(),
        DB_VERSION.to_string()
    )]
    VersionMismatch { version: u64 },
}

pub(crate) fn check_db_version_file<P: AsRef<Path>>(db_path: P) -> eyre::Result<()> {
    match fs::read_to_string(db_version_file_path(db_path)) {
        Ok(raw_version) => {
            let version =
                raw_version.parse::<u64>().map_err(|_| DatabaseVersionError::MalformedFile)?;
            if version != DB_VERSION {
                return Err(DatabaseVersionError::VersionMismatch { version }.into())
            }
        }
        Err(error) => {
            return match error.kind() {
                io::ErrorKind::NotFound => Err(DatabaseVersionError::MissingFile.into()),
                _ => Err(error.into()),
            }
        }
    }

    Ok(())
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
