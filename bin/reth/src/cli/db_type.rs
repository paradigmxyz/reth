//! A real or test database type

use std::sync::Arc;
use crate::dirs::{DataDirPath, MaybePlatformPath};
use reth_db::{
    init_db,
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_interfaces::db::LogLevel;
use reth_primitives::Chain;

/// A type that represents either a _real_ (represented by a path), or _test_ database, which will
/// use a [TempDatabase].
#[derive(Debug)]
pub enum DatabaseType {
    /// The real database type
    Real(MaybePlatformPath<DataDirPath>),
    /// The test database type
    Test,
}

/// The [Default] implementation for [DatabaseType] uses the _real_ variant, using the default
/// value for the inner [MaybePlatformPath].
impl Default for DatabaseType {
    fn default() -> Self {
        Self::Real(MaybePlatformPath::<DataDirPath>::default())
    }
}

impl DatabaseType {
    /// Creates a _test_ database
    pub fn test() -> Self {
        Self::Test
    }
}

/// Type that represents a [DatabaseType] and [LogLevel], used to build a database type
pub struct DatabaseBuilder {
    /// The database type
    db_type: DatabaseType,
}

impl DatabaseBuilder {
    /// Creates the [DatabaseBuilder] with the given [DatabaseType]
    pub fn new(db_type: DatabaseType) -> Self {
        Self { db_type }
    }

    /// Initializes and returns the test database. If the [DatabaseType] is test, the [LogLevel]
    /// and chain are not used.
    pub fn build_db(
        self,
        log_level: Option<LogLevel>,
        chain: Chain,
    ) -> eyre::Result<DatabaseInstance> {
        match self.db_type {
            DatabaseType::Test => Ok(DatabaseInstance::Test(create_test_rw_db())),
            DatabaseType::Real(path) => {
                let chain_dir = path.unwrap_or_chain_default(chain);

                tracing::info!(target: "reth::cli", path = ?chain_dir, "Opening database");
                Ok(DatabaseInstance::Real(Arc::new(init_db(chain_dir, log_level)?)))
            }
        }
    }
}

/// A constructed database type, without path information.
#[derive(Debug, Clone)]
pub enum DatabaseInstance {
    /// The test database
    Test(Arc<TempDatabase<DatabaseEnv>>),
    /// The right database
    Real(Arc<DatabaseEnv>),
}
