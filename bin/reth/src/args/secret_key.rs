use hex::encode as hex_encode;
use reth_network::config::rng_secret_key;
use secp256k1::{Error as SecretKeyBaseError, SecretKey};
use std::{
    fs::read_to_string,
    io,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Errors returned by loading a [`SecretKey`][secp256k1::SecretKey], including IO errors.
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum SecretKeyError {
    #[error(transparent)]
    SecretKeyDecodeError(#[from] SecretKeyBaseError),
    #[error("Failed to create parent directory {dir:?} for secret key: {error}")]
    FailedToCreateSecretParentDir { error: io::Error, dir: PathBuf },
    #[error("Failed to write secret key file {secret_file:?}: {error}")]
    FailedToWriteSecretKeyFile { error: io::Error, secret_file: PathBuf },
    #[error("Failed to read secret key file {secret_file:?}: {error}")]
    FailedToReadSecretKeyFile { error: io::Error, secret_file: PathBuf },
    #[error("Failed to access key file {secret_file:?}: {error}")]
    FailedToAccessKeyFile { error: io::Error, secret_file: PathBuf },
}

/// Attempts to load a [`SecretKey`] from a specified path. If no file exists there, then it
/// generates a secret key and stores it in the provided path. I/O errors might occur during write
/// operations in the form of a [`SecretKeyError`]
pub fn get_secret_key(secret_key_path: &Path) -> Result<SecretKey, SecretKeyError> {
    let exists = secret_key_path.try_exists();

    match exists {
        Ok(true) => {
            let contents = read_to_string(secret_key_path).map_err(|error| {
                SecretKeyError::FailedToReadSecretKeyFile {
                    error,
                    secret_file: secret_key_path.to_path_buf(),
                }
            })?;
            (contents.as_str().parse::<SecretKey>()).map_err(SecretKeyError::SecretKeyDecodeError)
        }
        Ok(false) => {
            if let Some(dir) = secret_key_path.parent() {
                // Create parent directory
                std::fs::create_dir_all(dir).map_err(|error| {
                    SecretKeyError::FailedToCreateSecretParentDir { error, dir: dir.to_path_buf() }
                })?;
            }

            let secret = rng_secret_key();
            let hex = hex_encode(secret.as_ref());
            std::fs::write(secret_key_path, hex).map_err(|error| {
                SecretKeyError::FailedToWriteSecretKeyFile {
                    error,
                    secret_file: secret_key_path.to_path_buf(),
                }
            })?;
            Ok(secret)
        }
        Err(error) => Err(SecretKeyError::FailedToAccessKeyFile {
            error,
            secret_file: secret_key_path.to_path_buf(),
        }),
    }
}
