//! Wrapper for `std::fs` methods
use crate::JwtError;
use std::{fs, path::Path};

type Result<T> = std::result::Result<T, JwtError>;

/// Wrapper for [`std::fs::read_to_string`].
pub fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    fs::read_to_string(path).map_err(|err| JwtError::read(err, path))
}
