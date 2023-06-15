use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::info;

/// The name of the file that contains the version of the database.
const DB_VERSION_FILE_NAME: &str = "db.version";
/// The version of the database stored in the [DB_VERSION_FILE_NAME] file in the same directory as
/// database. Example: `1`
const DB_VERSION: u64 = 1;

/// The name of the file that contains the version of the client.
const CLIENT_VERSION_FILE_NAME: &str = "client.version";
/// The version of the client stored in the [CLIENT_VERSION_FILE_NAME] file in the same directory as
/// database. Example: `0.1.0-e43455c2`
const CLIENT_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "-", env!("VERGEN_GIT_SHA"));

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

/// Updates a client version file with [CLIENT_VERSION_FILE_NAME] name.
///
/// If the version in the last line matches current client version [CLIENT_VERSION], don't do
/// anything. Otherwise, append new line containing [CLIENT_VERSION] with the current unix timestamp
/// in seconds.
///
/// This function will create a file if it does not exist.
pub(crate) fn update_client_version_file<P: AsRef<Path>>(db_path: P) -> eyre::Result<()> {
    let client_version_file = client_version_file_path(&db_path);
    let version_matches = match fs::read_to_string(&client_version_file) {
        Ok(content) => match content.lines().last() {
            Some(last_line) => {
                let version =
                        // "0.1.0-e43455c2 1686850247" -> ("0.1.0-e43455c2", "1686850247")
                        last_line.split_once(' ')
                            // "0.1.0-e43455c2" -> ("0.1.0", "e43455c2")
                            .and_then(|(version, _timestamp)| version.split_once('-'))
                            // ("0.1.0", "e43455c2") -> "0.1.0"
                            .map(|(version, _hash)| version);

                if let Some(version) = version {
                    version == CLIENT_VERSION
                } else {
                    info!(last_version = last_line, "Client version file is malformed");
                    fs::remove_file(&client_version_file)?;

                    false
                }
            }
            None => false,
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };

    if !version_matches {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(client_version_file)?;
        writeln!(
            file,
            "{CLIENT_VERSION} {}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
        )?;
    }

    Ok(())
}

fn client_version_file_path<P: AsRef<Path>>(db_path: P) -> PathBuf {
    db_path.as_ref().join(CLIENT_VERSION_FILE_NAME)
}
