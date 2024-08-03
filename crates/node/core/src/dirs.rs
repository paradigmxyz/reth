//! reth data directories.

use crate::{args::DatadirArgs, utils::parse_path};
use reth_chainspec::Chain;
use std::{
    env::VarError,
    fmt::{Debug, Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

/// Constructs a string to be used as a path for configuration and db paths.
pub fn config_path_prefix(chain: Chain) -> String {
    chain.to_string()
}

/// Returns the path to the reth data directory.
///
/// Refer to [`dirs_next::data_dir`] for cross-platform behavior.
pub fn data_dir() -> Option<PathBuf> {
    dirs_next::data_dir().map(|root| root.join("reth"))
}

/// Returns the path to the reth database.
///
/// Refer to [`dirs_next::data_dir`] for cross-platform behavior.
pub fn database_path() -> Option<PathBuf> {
    data_dir().map(|root| root.join("db"))
}

/// Returns the path to the reth configuration directory.
///
/// Refer to [`dirs_next::config_dir`] for cross-platform behavior.
pub fn config_dir() -> Option<PathBuf> {
    dirs_next::config_dir().map(|root| root.join("reth"))
}

/// Returns the path to the reth cache directory.
///
/// Refer to [`dirs_next::cache_dir`] for cross-platform behavior.
pub fn cache_dir() -> Option<PathBuf> {
    dirs_next::cache_dir().map(|root| root.join("reth"))
}

/// Returns the path to the reth logs directory.
///
/// Refer to [`dirs_next::cache_dir`] for cross-platform behavior.
pub fn logs_dir() -> Option<PathBuf> {
    cache_dir().map(|root| root.join("logs"))
}

/// Returns the path to the reth data dir.
///
/// The data dir should contain a subdirectory for each chain, and those chain directories will
/// include all information for that chain, such as the p2p secret.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct DataDirPath;

impl XdgPath for DataDirPath {
    fn resolve() -> Option<PathBuf> {
        data_dir()
    }
}

/// Returns the path to the reth logs directory.
///
/// Refer to [`dirs_next::cache_dir`] for cross-platform behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct LogsDir;

impl XdgPath for LogsDir {
    fn resolve() -> Option<PathBuf> {
        logs_dir()
    }
}

/// A small helper trait for unit structs that represent a standard path following the XDG
/// path specification.
pub trait XdgPath {
    /// Resolve the standard path.
    fn resolve() -> Option<PathBuf>;
}

/// A wrapper type that either parses a user-given path or defaults to an
/// OS-specific path.
///
/// The [`FromStr`] implementation supports shell expansions and common patterns such as `~` for the
/// home directory.
///
/// # Example
///
/// ```
/// use reth_node_core::dirs::{DataDirPath, PlatformPath};
/// use std::str::FromStr;
///
/// // Resolves to the platform-specific database path
/// let default: PlatformPath<DataDirPath> = PlatformPath::default();
/// // Resolves to `$(pwd)/my/path/to/datadir`
/// let custom: PlatformPath<DataDirPath> = PlatformPath::from_str("my/path/to/datadir").unwrap();
///
/// assert_ne!(default.as_ref(), custom.as_ref());
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct PlatformPath<D>(PathBuf, std::marker::PhantomData<D>);

impl<D> Display for PlatformPath<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl<D> Clone for PlatformPath<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), std::marker::PhantomData)
    }
}

impl<D: XdgPath> Default for PlatformPath<D> {
    fn default() -> Self {
        Self(
            D::resolve().expect("Could not resolve default path. Set one manually."),
            std::marker::PhantomData,
        )
    }
}

impl<D> FromStr for PlatformPath<D> {
    type Err = shellexpand::LookupError<VarError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse_path(s)?, std::marker::PhantomData))
    }
}

impl<D> AsRef<Path> for PlatformPath<D> {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl<D> From<PlatformPath<D>> for PathBuf {
    fn from(value: PlatformPath<D>) -> Self {
        value.0
    }
}

impl<D> PlatformPath<D> {
    /// Returns the path joined with another path
    pub fn join<P: AsRef<Path>>(&self, path: P) -> Self {
        Self(self.0.join(path), std::marker::PhantomData)
    }
}

impl<D> PlatformPath<D> {
    /// Converts the path to a `ChainPath` with the given `Chain`.
    pub fn with_chain(&self, chain: Chain, datadir_args: DatadirArgs) -> ChainPath<D> {
        // extract chain name
        let platform_path = self.platform_path_from_chain(chain);

        ChainPath::new(platform_path, chain, datadir_args)
    }

    fn platform_path_from_chain(&self, chain: Chain) -> Self {
        let chain_name = config_path_prefix(chain);
        let path = self.0.join(chain_name);
        Self(path, std::marker::PhantomData)
    }

    /// Map the inner path to a new type `T`.
    pub fn map_to<T>(&self) -> PlatformPath<T> {
        PlatformPath(self.0.clone(), std::marker::PhantomData)
    }
}

/// An Optional wrapper type around [`PlatformPath`].
///
/// This is useful for when a path is optional, such as the `--data-dir` flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaybePlatformPath<D>(Option<PlatformPath<D>>);

// === impl MaybePlatformPath ===

impl<D: XdgPath> MaybePlatformPath<D> {
    /// Returns the path if it is set, otherwise returns the default path for the given chain.
    pub fn unwrap_or_chain_default(&self, chain: Chain, datadir_args: DatadirArgs) -> ChainPath<D> {
        ChainPath(
            self.0
                .clone()
                .unwrap_or_else(|| PlatformPath::default().platform_path_from_chain(chain)),
            chain,
            datadir_args,
        )
    }

    /// Returns the default platform path for the specified [Chain].
    pub fn chain_default(chain: Chain) -> ChainPath<D> {
        PlatformPath::default().with_chain(chain, DatadirArgs::default())
    }

    /// Returns true if a custom path is set
    pub const fn is_some(&self) -> bool {
        self.0.is_some()
    }

    /// Returns the path if it is set, otherwise returns `None`.
    pub fn as_ref(&self) -> Option<&Path> {
        self.0.as_ref().map(|p| p.as_ref())
    }

    /// Returns the path if it is set, otherwise returns the default path, without any chain
    /// directory.
    pub fn unwrap_or_default(&self) -> PlatformPath<D> {
        self.0.clone().unwrap_or_default()
    }
}

impl<D: XdgPath> Display for MaybePlatformPath<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(path) = &self.0 {
            path.fmt(f)
        } else {
            // NOTE: this is a workaround for making it work with clap's `default_value_t` which
            // computes the default value via `Default -> Display -> FromStr`
            write!(f, "default")
        }
    }
}

impl<D> Default for MaybePlatformPath<D> {
    fn default() -> Self {
        Self(None)
    }
}

impl<D> FromStr for MaybePlatformPath<D> {
    type Err = shellexpand::LookupError<VarError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let p = match s {
            "default" => {
                // NOTE: this is a workaround for making it work with clap's `default_value_t` which
                // computes the default value via `Default -> Display -> FromStr`
                None
            }
            _ => Some(PlatformPath::from_str(s)?),
        };
        Ok(Self(p))
    }
}

impl<D> From<PathBuf> for MaybePlatformPath<D> {
    fn from(path: PathBuf) -> Self {
        Self(Some(PlatformPath(path, std::marker::PhantomData)))
    }
}

/// Wrapper type around `PlatformPath` that includes a `Chain`, used for separating reth data for
/// different networks.
///
/// If the chain is either mainnet, sepolia, or holesky, then the path will be:
///  * mainnet: `<DIR>/mainnet`
///  * sepolia: `<DIR>/sepolia`
///  * holesky: `<DIR>/holesky`
///
/// Otherwise, the path will be dependent on the chain ID:
///  * `<DIR>/<CHAIN_ID>`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainPath<D>(PlatformPath<D>, Chain, DatadirArgs);

impl<D> ChainPath<D> {
    /// Returns a new `ChainPath` given a `PlatformPath` and a `Chain`.
    pub const fn new(path: PlatformPath<D>, chain: Chain, datadir_args: DatadirArgs) -> Self {
        Self(path, chain, datadir_args)
    }

    /// Returns the path to the reth data directory for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>`
    pub fn data_dir(&self) -> &Path {
        self.0.as_ref()
    }

    /// Returns the path to the db directory for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>/db`
    pub fn db(&self) -> PathBuf {
        self.data_dir().join("db")
    }

    /// Returns the path to the static files directory for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>/static_files`
    pub fn static_files(&self) -> PathBuf {
        let datadir_args = &self.2;
        if let Some(static_files_path) = &datadir_args.static_files_path {
            static_files_path.to_path_buf()
        } else {
            self.data_dir().join("static_files")
        }
    }

    /// Returns the path to the reth p2p secret key for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>/discovery-secret`
    pub fn p2p_secret(&self) -> PathBuf {
        self.data_dir().join("discovery-secret")
    }

    /// Returns the path to the known peers file for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>/known-peers.json`
    pub fn known_peers(&self) -> PathBuf {
        self.data_dir().join("known-peers.json")
    }

    /// Returns the path to the blobstore directory for this chain where blobs of unfinalized
    /// transactions are stored.
    ///
    /// `<DIR>/<CHAIN_ID>/blobstore`
    pub fn blobstore(&self) -> PathBuf {
        self.data_dir().join("blobstore")
    }

    /// Returns the path to the local transactions backup file
    ///
    /// `<DIR>/<CHAIN_ID>/txpool-transactions-backup.rlp`
    pub fn txpool_transactions(&self) -> PathBuf {
        self.data_dir().join("txpool-transactions-backup.rlp")
    }

    /// Returns the path to the config file for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>/reth.toml`
    pub fn config(&self) -> PathBuf {
        self.data_dir().join("reth.toml")
    }

    /// Returns the path to the jwtsecret file for this chain.
    ///
    /// `<DIR>/<CHAIN_ID>/jwt.hex`
    pub fn jwt(&self) -> PathBuf {
        self.data_dir().join("jwt.hex")
    }
}

impl<D> AsRef<Path> for ChainPath<D> {
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}

impl<D> Display for ChainPath<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<D> From<ChainPath<D>> for PathBuf {
    fn from(value: ChainPath<D>) -> Self {
        value.0.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maybe_data_dir_path() {
        let path = MaybePlatformPath::<DataDirPath>::default();
        let path = path.unwrap_or_chain_default(Chain::mainnet(), DatadirArgs::default());
        assert!(path.as_ref().ends_with("reth/mainnet"), "{path:?}");

        let db_path = path.db();
        assert!(db_path.ends_with("reth/mainnet/db"), "{db_path:?}");

        let path = MaybePlatformPath::<DataDirPath>::from_str("my/path/to/datadir").unwrap();
        let path = path.unwrap_or_chain_default(Chain::mainnet(), DatadirArgs::default());
        assert!(path.as_ref().ends_with("my/path/to/datadir"), "{path:?}");
    }

    #[test]
    fn test_maybe_testnet_datadir_path() {
        let path = MaybePlatformPath::<DataDirPath>::default();
        let path = path.unwrap_or_chain_default(Chain::holesky(), DatadirArgs::default());
        assert!(path.as_ref().ends_with("reth/holesky"), "{path:?}");

        let path = MaybePlatformPath::<DataDirPath>::default();
        let path = path.unwrap_or_chain_default(Chain::sepolia(), DatadirArgs::default());
        assert!(path.as_ref().ends_with("reth/sepolia"), "{path:?}");
    }
}
