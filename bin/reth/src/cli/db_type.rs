//! A real or test database type

use crate::dirs::{ChainPath, DataDirPath, MaybePlatformPath};
use reth_db::{
    init_db,
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_interfaces::db::LogLevel;
use reth_primitives::Chain;
use std::{str::FromStr, sync::Arc};

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

    /// Initializes and returns the [DatabaseInstance] depending on the current database type. If
    /// the [DatabaseType] is test, the [LogLevel] is not used.
    ///
    /// If the [DatabaseType] is test, then the [ChainPath] constructed will be derived from the db
    /// path of the [TempDatabase] and the given chain.
    pub fn build_db(
        self,
        log_level: Option<LogLevel>,
        chain: Chain,
    ) -> eyre::Result<DatabaseInstance> {
        match self.db_type {
            DatabaseType::Test => {
                let db = create_test_rw_db();
                let db_path_str = db.path().to_str().expect("Path is not valid unicode");
                let path = MaybePlatformPath::<DataDirPath>::from_str(db_path_str)
                    .expect("Path is not valid");
                let data_dir = path.unwrap_or_chain_default(chain);

                Ok(DatabaseInstance::Test { db, data_dir })
            }
            DatabaseType::Real(path) => {
                let data_dir = path.unwrap_or_chain_default(chain);

                tracing::info!(target: "reth::cli", path = ?data_dir, "Opening database");
                let db = Arc::new(init_db(data_dir.clone(), log_level)?);
                Ok(DatabaseInstance::Real { db, data_dir })
            }
        }
    }
}

/// A constructed database type, with a [ChainPath].
#[derive(Debug, Clone)]
pub enum DatabaseInstance {
    /// The test database
    Test {
        /// The database
        db: Arc<TempDatabase<DatabaseEnv>>,
        /// The data dir
        data_dir: ChainPath<DataDirPath>,
    },
    /// The real database
    Real {
        /// The database
        db: Arc<DatabaseEnv>,
        /// The data dir
        data_dir: ChainPath<DataDirPath>,
    },
}

impl DatabaseInstance {
    /// Returns the data dir for this database instance
    pub fn data_dir(&self) -> &ChainPath<DataDirPath> {
        match self {
            Self::Test { data_dir, .. } => data_dir,
            Self::Real { data_dir, .. } => data_dir,
        }
    }
}
