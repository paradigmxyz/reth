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

#[derive(thiserror::Error, Debug)]
enum ClientVersionError {
    #[error(
        "Reth (v{}) is unable to determine the version of the client, file is missing.",
        CLIENT_VERSION.to_string()
    )]
    MissingFile,
    #[error(
        "Reth (v{}) is unable to determine the version of the client, file is malformed.",
        CLIENT_VERSION.to_string()
    )]
    MalformedFile { last_version: String },
    #[error(transparent)]
    IOError(#[from] io::Error),
}

/// Updates a client version file with [CLIENT_VERSION_FILE_NAME] name.
///
/// If the version in the last line matches current client version [CLIENT_VERSION], don't do
/// anything. Otherwise, append new line containing [CLIENT_VERSION] with the current unix timestamp
/// in seconds.
///
/// This function will create a file if it does not exist.
pub(crate) fn update_client_version_file<P: AsRef<Path>>(db_path: P) -> eyre::Result<()> {
    let version_file = client_version_file_path(&db_path);
    let version_matches = match client_version_matches(&version_file) {
        Ok(matches) => matches,
        Err(ClientVersionError::MissingFile) => false,
        Err(ClientVersionError::MalformedFile { last_version }) => {
            info!(last_version, "Client version file is malformed");
            false
        }
        Err(ClientVersionError::IOError(err)) => return Err(err.into()),
    };

    if !version_matches {
        let mut file =
            fs::OpenOptions::new().create(true).write(true).append(true).open(version_file)?;
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

fn client_version_matches<P: AsRef<Path>>(version_file: P) -> Result<bool, ClientVersionError> {
    match fs::read_to_string(&version_file) {
        Ok(content) => Ok(match content.lines().last() {
            Some(last_line) => {
                let version =
                    // "0.1.0-e43455c2 1686850247" -> ("0.1.0-e43455c2", "1686850247")
                    last_line.split_once(' ')
                        // ("0.1.0-e43455c2", "1686850247") -> "0.1.0-e43455c2"
                        .and_then(|(version, _timestamp)| if version.contains('-') {
                            Some(version)
                        } else {
                            None
                        }).ok_or(ClientVersionError::MalformedFile {
                        last_version: last_line.to_string(),
                    })?;

                version == CLIENT_VERSION
            }
            None => false,
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(ClientVersionError::MissingFile),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::versioning::client::{
        client_version_file_path, client_version_matches, ClientVersionError, CLIENT_VERSION,
    };
    use assert_matches::assert_matches;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tempfile::tempdir;

    #[test]
    fn missing_file() {
        let dir = tempdir().unwrap();

        let client_version_matches = client_version_matches(client_version_file_path(&dir));
        assert_matches!(client_version_matches, Err(ClientVersionError::MissingFile));
    }

    #[test]
    fn malformed_file() {
        let dir = tempdir().unwrap();
        let contents = "invalid-version".to_string();
        fs::write(client_version_file_path(&dir), &contents).unwrap();

        let client_version_matches = client_version_matches(client_version_file_path(&dir));
        assert_matches!(
            client_version_matches,
            Err(ClientVersionError::MalformedFile { last_version }) if last_version == contents
        );
    }

    #[test]
    fn version_matches() {
        let dir = tempdir().unwrap();
        fs::write(
            client_version_file_path(&dir),
            format!(
                "{CLIENT_VERSION} {}",
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
            ),
        )
        .unwrap();

        let client_version_matches = client_version_matches(client_version_file_path(&dir));
        assert_matches!(client_version_matches, Ok(true));
    }

    #[test]
    fn version_doesnt_match() {
        let dir = tempdir().unwrap();
        fs::write(
            client_version_file_path(&dir),
            format!(
                "0.0.0-aaaaaaaa {}",
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
            ),
        )
        .unwrap();

        let client_version_matches = client_version_matches(client_version_file_path(&dir));
        assert_matches!(client_version_matches, Ok(false));
    }
}
