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

/// Attempts to load a [`SecretKey`] from a specified path. If no file exists there, then it
/// generates a secret key and stores it in the provided path. I/O errors might occur during write
/// operations in the form of a [`SecretKeyError`]
pub fn get_secret_key(secret_key_path: &Path) -> Result<SecretKey, SecretKeyError> {
    let exists = secret_key_path.try_exists();

    match exists {
        Ok(true) => {
            let contents = read_to_string(secret_key_path)?;
            (contents.as_str().parse::<SecretKey>()).map_err(SecretKeyError::SecretKeyDecodeError)
        }
        Ok(false) => {
            if let Some(dir) = secret_key_path.parent() {
                // Create parent directory
                std::fs::create_dir_all(dir)?
            }

            let secret = rng_secret_key();
            let hex = hex_encode(secret.as_ref());
            std::fs::write(secret_key_path, hex)?;
            Ok(secret)
        }
        Err(e) => Err(SecretKeyError::IOError(e)),
    }
}
