//! reth data directories.
use reth_cli_utils::parse_path;
use std::{
    env::VarError,
    fmt::{Debug, Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

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

/// Returns the path to the reth database.
///
/// Refer to [dirs_next::data_dir] for cross-platform behavior.
#[derive(Default, Debug, Clone)]
pub struct DbPath;

impl XdgPath for DbPath {
    fn resolve() -> Option<PathBuf> {
        database_path()
    }
}

/// Returns the path to the default reth configuration file.
///
/// Refer to [dirs_next::config_dir] for cross-platform behavior.
#[derive(Default, Debug, Clone)]
pub struct ConfigPath;

impl XdgPath for ConfigPath {
    fn resolve() -> Option<PathBuf> {
        config_dir().map(|p| p.join("reth.toml"))
    }
}

/// Returns the path to the reth logs directory.
///
/// Refer to [dirs_next::cache_dir] for cross-platform behavior.
#[derive(Default, Debug, Clone)]
pub struct LogsDir;

impl XdgPath for LogsDir {
    fn resolve() -> Option<PathBuf> {
        logs_dir()
    }
}

/// A small helper trait for unit structs that represent a standard path following the XDG
/// path specification.
trait XdgPath {
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
/// ```rs
/// let default: StandardPath<DbPath> = StandardPath::default();
/// let custom = StandardPath::from_str("my/path/to/db").unwrap();
///
/// println!("Default db path: {default}, custom db path: {custom}");
/// ```
#[derive(Clone, Debug)]
pub struct StandardPath<D>(PathBuf, std::marker::PhantomData<D>);

impl<D> Display for StandardPath<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl<D: XdgPath> Default for StandardPath<D> {
    fn default() -> Self {
        Self(
            D::resolve().expect("Could not resolve default path. Set one manually."),
            std::marker::PhantomData,
        )
    }
}

impl<D> FromStr for StandardPath<D> {
    type Err = shellexpand::LookupError<VarError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse_path(s)?, std::marker::PhantomData))
    }
}

impl<D> AsRef<Path> for StandardPath<D> {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}
