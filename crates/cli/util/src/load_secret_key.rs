use reth_fs_util::{self as fs, FsPathError};
use secp256k1::{Error as SecretKeyBaseError, SecretKey};
use std::{
    io,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Convenience function to create a new random [`SecretKey`]
pub fn rng_secret_key() -> SecretKey {
    SecretKey::new(&mut rand_08::thread_rng())
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

    /// Invalid hex string format.
    #[error("invalid hex string: {0}")]
    InvalidHexString(String),
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

/// Parses a [`SecretKey`] from a hex string.
///
/// The hex string can optionally start with "0x".
pub fn parse_secret_key_from_hex(hex_str: &str) -> Result<SecretKey, SecretKeyError> {
    // Remove "0x" prefix if present
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    
    // Decode the hex string
    let bytes = alloy_primitives::hex::decode(hex_str)
        .map_err(|e| SecretKeyError::InvalidHexString(e.to_string()))?;
    
    // Parse into SecretKey
    SecretKey::from_slice(&bytes).map_err(SecretKeyError::SecretKeyDecodeError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_secret_key_from_hex_without_prefix() {
        // Valid 32-byte hex string (64 characters)
        let hex = "4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f";
        let result = parse_secret_key_from_hex(hex);
        assert!(result.is_ok());
        
        let secret_key = result.unwrap();
        assert_eq!(alloy_primitives::hex::encode(secret_key.secret_bytes()), hex);
    }

    #[test]
    fn test_parse_secret_key_from_hex_with_0x_prefix() {
        // Valid 32-byte hex string with 0x prefix
        let hex = "0x4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f";
        let result = parse_secret_key_from_hex(hex);
        assert!(result.is_ok());
        
        let secret_key = result.unwrap();
        let expected = "4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f";
        assert_eq!(alloy_primitives::hex::encode(secret_key.secret_bytes()), expected);
    }

    #[test]
    fn test_parse_secret_key_from_hex_invalid_length() {
        // Invalid length (not 32 bytes)
        let hex = "4c0883a69102937d";
        let result = parse_secret_key_from_hex(hex);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_secret_key_from_hex_invalid_chars() {
        // Invalid hex characters
        let hex = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        let result = parse_secret_key_from_hex(hex);
        assert!(result.is_err());
        
        if let Err(SecretKeyError::InvalidHexString(_)) = result {
            // Expected error type
        } else {
            panic!("Expected InvalidHexString error");
        }
    }

    #[test]
    fn test_parse_secret_key_from_hex_empty() {
        let hex = "";
        let result = parse_secret_key_from_hex(hex);
        assert!(result.is_err());
    }
}
