use reth_fs_util::{self as fs, FsPathError};
use secp256k1::{Error as SecretKeyBaseError, SecretKey};
use std::{
    io,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Convenience function to create a new random [`SecretKey`]
pub fn rng_secret_key() -> SecretKey {
    SecretKey::new(&mut rand::thread_rng())
}

/// Errors returned by loading a [`SecretKey`], including IO errors.
#[derive(Error, Debug)]
pub enum SecretKeyError {
    /// Error encountered during decoding of the secret key.
    #[error(transparent)]
    SecretKeyDecodeError(#[from] SecretKeyBaseError),

    /// Error related to file system path operations.
    #[error(transparent)]
    SecretKeyFsPathError(#[from] FsPathError),

    /// Represents an error when failed to access the key file.
    #[error("failed to access key file {secret_file:?}: {error}")]
    FailedToAccessKeyFile {
        /// The encountered IO error.
        error: io::Error,
        /// Path to the secret key file.
        secret_file: PathBuf,
    },
}

/// Attempts to load a [`SecretKey`] from a specified path. If no file exists there, then it
/// generates a secret key and stores it in the provided path. I/O errors might occur during write
/// operations in the form of a [`SecretKeyError`]
pub fn get_secret_key(secret_key_path: &Path) -> Result<SecretKey, SecretKeyError> {
    let exists = secret_key_path.try_exists();

    match exists {
        Ok(true) => {
            let contents = fs::read_to_string(secret_key_path)?;
            Ok(contents.as_str().parse().map_err(SecretKeyError::SecretKeyDecodeError)?)
        }
        Ok(false) => {
            if let Some(dir) = secret_key_path.parent() {
                // Create parent directory
                fs::create_dir_all(dir)?;
            }

            let secret = rng_secret_key();
            let hex = alloy_primitives::hex::encode(secret.as_ref());
            fs::write(secret_key_path, hex)?;
            Ok(secret)
        }
        Err(error) => Err(SecretKeyError::FailedToAccessKeyFile {
            error,
            secret_file: secret_key_path.to_path_buf(),
        }),
    }
}
