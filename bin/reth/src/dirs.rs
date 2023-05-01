//! reth data directories.
use reth_primitives::Chain;
use reth_staged_sync::utils::parse_path;
use std::{
    env::VarError,
    fmt::{Debug, Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

/// Constructs a string to be used as a path for configuration and db paths.
pub fn config_path_prefix(chain: Chain) -> String {
    if chain == Chain::mainnet() {
        "mainnet".to_string()
    } else if chain == Chain::goerli() {
        "goerli".to_string()
    } else if chain == Chain::sepolia() {
        "sepolia".to_string()
    } else {
        chain.id().to_string()
    }
}

/// Returns the path to the reth data directory.
///
/// Refer to [dirs_next::data_dir] for cross-platform behavior.
pub fn data_dir() -> Option<PathBuf> {
    dirs_next::data_dir().map(|root| root.join("reth"))
}

/// Returns the path to the reth database.
///
/// Refer to [dirs_next::data_dir] for cross-platform behavior.
pub fn database_path() -> Option<PathBuf> {
    data_dir().map(|root| root.join("db"))
}

/// Returns the path to the reth configuration directory.
///
/// Refer to [dirs_next::config_dir] for cross-platform behavior.
pub fn config_dir() -> Option<PathBuf> {
    dirs_next::config_dir().map(|root| root.join("reth"))
}

/// Returns the path to the reth cache directory.
///
/// Refer to [dirs_next::cache_dir] for cross-platform behavior.
pub fn cache_dir() -> Option<PathBuf> {
    dirs_next::cache_dir().map(|root| root.join("reth"))
}

/// Returns the path to the reth logs directory.
///
/// Refer to [dirs_next::cache_dir] for cross-platform behavior.
pub fn logs_dir() -> Option<PathBuf> {
    cache_dir().map(|root| root.join("logs"))
}

/// Returns the path to the reth data dir.
///
/// The data dir should contain a subdirectory for each chain, and those chain directories will
/// include all information for that chain, such as the p2p secret.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct DataDirPath;

impl XdgPath for DataDirPath {
    fn resolve() -> Option<PathBuf> {
        data_dir()
    }
}

/// Returns the path to the default reth configuration file.
///
/// Refer to [dirs_next::data_dir] for cross-platform behavior.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct ConfigPath;

impl XdgPath for ConfigPath {
    fn resolve() -> Option<PathBuf> {
        data_dir().map(|p| p.join("reth.toml"))
    }
}

/// Returns the path to the reth logs directory.
///
/// Refer to [dirs_next::cache_dir] for cross-platform behavior.
#[derive(Default, Debug, Clone)]
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
/// The [FromStr] implementation supports shell expansions and common patterns such as `~` for the
/// home directory.
///
/// # Example
///
/// ```
/// use reth::dirs::{PlatformPath, DataDirPath};
/// use std::str::FromStr;
///
/// // Resolves to the platform-specific database path
/// let default: PlatformPath<DataDirPath> = PlatformPath::default();
/// // Resolves to `$(pwd)/my/path/to/datadir`
/// let custom: PlatformPath<DataDirPath> = PlatformPath::from_str("my/path/to/datadir").unwrap();
///
/// assert_ne!(default.as_ref(), custom.as_ref());
/// ```
#[derive(Debug, PartialEq)]
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
    pub fn join<P: AsRef<Path>>(&self, path: P) -> PlatformPath<D> {
        PlatformPath::<D>(self.0.join(path), std::marker::PhantomData)
    }
}

impl<D> PlatformPath<D> {
    /// Converts the path to a `ChainPath` with the given `Chain`.
    pub fn with_chain(&self, chain: Chain) -> ChainPath<D> {
        // extract chain name
        let chain_name = config_path_prefix(chain);

        let path = self.0.join(chain_name);

        let platform_path = PlatformPath::<D>(path, std::marker::PhantomData);
        ChainPath::new(platform_path, chain)
    }
}

/// An Optional wrapper type around [PlatformPath].
///
/// This is useful for when a path is optional, such as the `--data-dir` flag.
#[derive(Clone, Debug, PartialEq)]
pub struct MaybePlatformPath<D>(Option<PlatformPath<D>>);

// === impl MaybePlatformPath ===

impl<D: XdgPath> MaybePlatformPath<D> {
    /// Returns the path if it is set, otherwise returns the default path for the given chain.
    pub fn unwrap_or_chain_default(&self, chain: Chain) -> ChainPath<D> {
        ChainPath(
            self.0.clone().unwrap_or_else(|| PlatformPath::default().with_chain(chain).0),
            chain,
        )
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

/// Wrapper type around PlatformPath that includes a `Chain`, used for separating reth data for
/// different networks.
///
/// If the chain is either mainnet, goerli, or sepolia, then the path will be:
///  * mainnet: `<DIR>/mainnet`
///  * goerli: `<DIR>/goerli`
///  * sepolia: `<DIR>/sepolia`
/// Otherwise, the path will be dependent on the chain ID:
///  * `<DIR>/<CHAIN_ID>`
#[derive(Clone, Debug, PartialEq)]
pub struct ChainPath<D>(PlatformPath<D>, Chain);

impl<D> ChainPath<D> {
    /// Returns a new `ChainPath` given a `PlatformPath` and a `Chain`.
    pub fn new(path: PlatformPath<D>, chain: Chain) -> Self {
        Self(path, chain)
    }

    /// Returns the path to the db directory for this chain.
    pub fn db_path(&self) -> PathBuf {
        self.0.join("db").into()
    }

    /// Returns the path to the reth secret key directory for this chain.
    pub fn p2p_path(&self) -> P2PPath<D> {
        P2PPath(self.0.join("p2p"))
    }

    /// Returns the path to the net directory for this chain.
    pub fn net_path(&self) -> NetPath<D> {
        NetPath(self.0.join("net"))
    }

    /// Returns the path to the jwtsecret directory for this chain.
    pub fn jwt_path(&self) -> JwtSecretPath<D> {
        JwtSecretPath(self.0.join("jwtsecret"))
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

/// A type representing a path to the reth net directory.
#[derive(Clone, Debug, PartialEq)]
pub struct NetPath<D>(PlatformPath<D>);

impl<D> NetPath<D> {
    /// Returns the path to the default reth known peers file for this net directory.
    pub fn known_peers_path(&self) -> PathBuf {
        self.0.join("known_peers.json").into()
    }
}

/// A type representing a path to the reth jwtsecret directory.
#[derive(Clone, Debug, PartialEq)]
pub struct JwtSecretPath<D>(PlatformPath<D>);

impl<D> JwtSecretPath<D> {
    /// Returns the path to the default reth jwtsecret file for this jwtsecret directory.
    pub fn jwtsecret_path(&self) -> PathBuf {
        self.0.join("jwt.hex").into()
    }
}

/// A type represeting a path to the reth p2p secret directory.
#[derive(Clone, Debug, PartialEq)]
pub struct P2PPath<D>(PlatformPath<D>);

impl<D> P2PPath<D> {
    /// Returns the path to the default reth p2p secret key file for this p2p directory.
    pub fn p2p_secret_path(&self) -> PathBuf {
        self.0.join("secret").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maybe_data_dir_path() {
        let path = MaybePlatformPath::<DataDirPath>::default();
        let path = path.unwrap_or_chain_default(Chain::mainnet());
        assert!(path.as_ref().ends_with("reth/mainnet"), "{:?}", path);

        let db_path = path.db_path();
        assert!(path.as_ref().ends_with("reth/mainnet/db"), "{:?}", db_path);

        let path = MaybePlatformPath::<DataDirPath>::from_str("my/path/to/db").unwrap();
        let path = path.unwrap_or_chain_default(Chain::mainnet());
        assert!(path.as_ref().ends_with("my/path/to/db"), "{:?}", path);
    }

    #[test]
    fn test_maybe_config_platform_path() {
        let path = MaybePlatformPath::<ConfigPath>::default();
        let path = path.unwrap_or_chain_default(Chain::mainnet());
        assert!(path.as_ref().ends_with("reth/mainnet/reth.toml"), "{:?}", path);

        let path = MaybePlatformPath::<ConfigPath>::from_str("my/path/to/reth.toml").unwrap();
        let path = path.unwrap_or_chain_default(Chain::mainnet());
        assert!(path.as_ref().ends_with("my/path/to/reth.toml"), "{:?}", path);
    }
}
