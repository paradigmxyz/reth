use crate::dirs::{PlatformPath, SecretKeyPath};
use hex::encode as hex_encode;
use reth_network::config::rng_secret_key;
use secp256k1::{Error as SecretKeyBaseError, SecretKey};
use std::{fs::read_to_string, path::Path};
use thiserror::Error;

/// Errors returned by loading a [`SecretKey`][secp256k1::SecretKey], including IO errors.
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum SecretKeyError {
    #[error(transparent)]
    SecretKeyDecodeError(#[from] SecretKeyBaseError),
    #[error("An I/O error occurred: {0}")]
    IOError(#[from] std::io::Error),
}

/// Attempts to load a [`SecretKey`] from a specified path. If no file exists
/// there, then it generates a secret key and stores it in the default path. I/O
/// errors might occur during write operations in the form of a
/// [`SecretKeyError`]
pub fn get_secret_key(secret_key_path: impl AsRef<Path>) -> Result<SecretKey, SecretKeyError> {
    let fpath = secret_key_path.as_ref();
    let exists = fpath.try_exists();

    match exists {
        Ok(true) => {
            let contents = read_to_string(fpath)?;
            (contents.as_str().parse::<SecretKey>()).map_err(SecretKeyError::SecretKeyDecodeError)
        }
        Ok(false) => {
            let default_path = PlatformPath::<SecretKeyPath>::default();
            let fpath = default_path.as_ref();

            if let Some(dir) = fpath.parent() {
                // Create parent directory
                std::fs::create_dir_all(dir)?
            }

            let secret = rng_secret_key();
            let hex = hex_encode(secret.as_ref());
            std::fs::write(fpath, hex)?;
            Ok(secret)
        }
        Err(e) => Err(SecretKeyError::IOError(e)),
    }
}
