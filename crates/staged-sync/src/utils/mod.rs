//! Utility functions.
use reth_primitives::{BlockHashOrNumber, H256};
use std::{
    env::VarError,
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    str::FromStr,
};
use walkdir::{DirEntry, WalkDir};

use self::error::SocketParsingError;

/// Utilities for parsing chainspecs
pub mod chainspec;

/// Custom error type while parsing socket addresses.
pub mod error;

/// Utilities for initializing parts of the chain
pub mod init;

/// Finds all files in a directory with a given postfix.
pub fn find_all_files_with_postfix(path: &Path, postfix: &str) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(postfix))
        .map(DirEntry::into_path)
        .collect::<Vec<PathBuf>>()
}

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, shellexpand::LookupError<VarError>> {
    shellexpand::full(value).map(|path| PathBuf::from(path.into_owned()))
}

/// Parse [BlockHashOrNumber]
pub fn hash_or_num_value_parser(value: &str) -> Result<BlockHashOrNumber, eyre::Error> {
    match H256::from_str(value) {
        Ok(hash) => Ok(BlockHashOrNumber::Hash(hash)),
        Err(_) => Ok(BlockHashOrNumber::Number(value.parse()?)),
    }
}

/// Parse a [SocketAddr] from a `str`.
///
/// The following formats are checked:
///
/// - If the value can be parsed as a `u16` or starts with `:` it is considered a port, and the
/// hostname is set to `localhost`.
/// - If the value contains `:` it is assumed to be the format `<host>:<port>`
/// - Otherwise it is assumed to be a hostname
///
/// An error is returned if the value is empty.
pub fn parse_socket_address(value: &str) -> Result<SocketAddr, SocketParsingError> {
    if value.is_empty() {
        return Err(SocketParsingError::Empty)
    }

    if value.starts_with(":") {
        ("localhost", value.strip_prefix(":").unwrap().parse::<u16>().unwrap()).to_socket_addrs()
    } else if value.contains(":") {
        value.to_socket_addrs()
    } else if let Ok(port) = value.parse::<u16>() {
        ("localhost", port).to_socket_addrs()
    } else {
        (value, 9000).to_socket_addrs()
    }?
    .next()
    .ok_or_else(|| SocketParsingError::Parse(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_socket_addresses() {
        for value in ["localhost:9000", ":9000", "9000", "localhost"] {
            let socket_addr = parse_socket_address(value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), 9000);
        }
    }
}
