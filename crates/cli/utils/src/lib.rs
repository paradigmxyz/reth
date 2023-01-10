#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Utility functions.
use reth_primitives::{BlockHashOrNumber, H256};
use std::{
    env::VarError,
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    str::FromStr,
};
use walkdir::{DirEntry, WalkDir};

/// Utilities for parsing chainspecs
pub mod chainspec;

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
pub fn parse_socket_address(value: &str) -> Result<SocketAddr, eyre::Error> {
    if value.is_empty() {
        eyre::bail!("Cannot parse socket address from an empty string");
    }

    if value.starts_with(':') || value.parse::<u16>().is_ok() {
        ("localhost", 9000).to_socket_addrs()
    } else if value.contains(':') {
        value.to_socket_addrs()
    } else {
        (value, 9000).to_socket_addrs()
    }?
    .next()
    .ok_or_else(|| eyre::eyre!("Could not parse socket address from {}", value))
}

/// Tracing utility
pub mod reth_tracing {
    use tracing::Subscriber;
    use tracing_subscriber::{prelude::*, EnvFilter};

    /// Tracing modes
    pub enum TracingMode {
        /// Enable all traces.
        All,
        /// Enable debug traces.
        Debug,
        /// Enable info traces.
        Info,
        /// Enable warn traces.
        Warn,
        /// Enable error traces.
        Error,
        /// Disable tracing.
        Silent,
    }

    impl TracingMode {
        fn into_env_filter(self) -> EnvFilter {
            match self {
                Self::All => EnvFilter::new("reth=trace"),
                Self::Debug => EnvFilter::new("reth=debug"),
                Self::Info => EnvFilter::new("reth=info"),
                Self::Warn => EnvFilter::new("reth=warn"),
                Self::Error => EnvFilter::new("reth=error"),
                Self::Silent => EnvFilter::new(""),
            }
        }
    }

    impl From<u8> for TracingMode {
        fn from(value: u8) -> Self {
            match value {
                0 => Self::Error,
                1 => Self::Warn,
                2 => Self::Info,
                3 => Self::Debug,
                _ => Self::All,
            }
        }
    }

    /// Build subscriber
    // TODO: JSON/systemd support
    pub fn build_subscriber(mods: TracingMode) -> impl Subscriber {
        // TODO: Auto-detect
        let no_color = std::env::var("RUST_LOG_STYLE").map(|val| val == "never").unwrap_or(false);
        let with_target = std::env::var("RUST_LOG_TARGET").map(|val| val != "0").unwrap_or(false);

        // Take env over config
        let filter = if std::env::var(EnvFilter::DEFAULT_ENV).unwrap_or_default().is_empty() {
            mods.into_env_filter()
        } else {
            EnvFilter::from_default_env()
        };

        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_ansi(!no_color).with_target(with_target))
            .with(filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_socket_addresses() {
        for value in ["localhost:9000", ":9000", "9000", "localhost"] {
            let socket_addr = parse_socket_address(value)
                .expect(&format!("could not parse socket address: {}", value));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), 9000);
        }
    }
}
