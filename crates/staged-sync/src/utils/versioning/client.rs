use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::info;

/// The name of the file that contains the version of the client.
const CLIENT_VERSION_FILE_NAME: &str = "client.version";
/// The version of the client stored in the [CLIENT_VERSION_FILE_NAME] file in the same directory as
/// database. Example: `0.1.0-e43455c2`
pub(crate) const CLIENT_VERSION: &str =
    concat!(env!("CARGO_PKG_VERSION"), "-", env!("VERGEN_GIT_SHA"));

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
