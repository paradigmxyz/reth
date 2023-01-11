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
    use std::path::Path;
    use tracing::Subscriber;
    use tracing_subscriber::{
        filter::Directive, prelude::*, registry::LookupSpan, EnvFilter, Layer, Registry,
    };

    /// A boxed tracing [Layer].
    pub type BoxedLayer<S> = Box<dyn Layer<S> + Send + Sync>;

    /// Initializes a new [Subscriber] based on the given layers.
    pub fn init(layers: Vec<BoxedLayer<Registry>>) {
        tracing_subscriber::registry().with(layers).init();
    }

    /// Builds a new tracing layer that writes to stdout.
    ///
    /// The events are filtered by `default_directive`, unless overriden by `RUST_LOG`.
    ///
    /// Colors can be disabled with `RUST_LOG_STYLE=never`, and event targets can be displayed with
    /// `RUST_LOG_TARGET=1`.
    pub fn stdout<S>(default_directive: impl Into<Directive>) -> BoxedLayer<S>
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        // TODO: Auto-detect
        let with_ansi = std::env::var("RUST_LOG_STYLE").map(|val| val != "never").unwrap_or(true);
        let with_target = std::env::var("RUST_LOG_TARGET").map(|val| val != "0").unwrap_or(false);

        let filter =
            EnvFilter::builder().with_default_directive(default_directive.into()).from_env_lossy();

        tracing_subscriber::fmt::layer()
            .with_ansi(with_ansi)
            .with_target(with_target)
            .with_filter(filter)
            .boxed()
    }

    /// Builds a new tracing layer that appends to a log file.
    ///
    /// The events are filtered by `directive`.
    ///
    /// The boxed layer and a guard is returned. When the guard is dropped the buffer for the log
    /// file is immediately flushed to disk. Any events after the guard is dropped may be missed.
    pub fn file<S>(
        directive: impl Into<Directive>,
        dir: impl AsRef<Path>,
        file_name: impl AsRef<Path>,
    ) -> (BoxedLayer<S>, tracing_appender::non_blocking::WorkerGuard)
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let (writer, guard) =
            tracing_appender::non_blocking(tracing_appender::rolling::never(dir, file_name));
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(EnvFilter::default().add_directive(directive.into()))
            .boxed();

        (layer, guard)
    }

    /// Builds a new tracing layer that writes events to journald.
    ///
    /// The events are filtered by `directive`.
    ///
    /// If the layer cannot connect to journald for any reason this function will return an error.
    pub fn journald<S>(directive: impl Into<Directive>) -> std::io::Result<BoxedLayer<S>>
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        Ok(tracing_journald::layer()?
            .with_filter(EnvFilter::default().add_directive(directive.into()))
            .boxed())
    }
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
