//! reth data directories.
use crate::util::parse_path;
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

/// A wrapper type that either parses a user-given path for the reth database or defaults to an
/// OS-specific path.
#[derive(Clone, Debug)]
pub struct DbPath(PathBuf);

impl Display for DbPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl Default for DbPath {
    fn default() -> Self {
        Self(database_path().expect("Could not determine default database path. Set one manually."))
    }
}

impl FromStr for DbPath {
    type Err = shellexpand::LookupError<VarError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse_path(s)?))
    }
}

impl AsRef<Path> for DbPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

/// A wrapper type that either parses a user-given path for the reth config or defaults to an
/// OS-specific path.
#[derive(Clone, Debug)]
pub struct ConfigPath(PathBuf);

impl Display for ConfigPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl Default for ConfigPath {
    fn default() -> Self {
        Self(
            config_dir()
                .expect("Could not determine default database path. Set one manually.")
                .join("reth.toml"),
        )
    }
}

impl FromStr for ConfigPath {
    type Err = shellexpand::LookupError<VarError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(parse_path(s)?))
    }
}

impl AsRef<Path> for ConfigPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}
