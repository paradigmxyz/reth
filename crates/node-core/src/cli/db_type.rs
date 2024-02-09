//! A real or test database type

use crate::dirs::{ChainPath, DataDirPath, MaybePlatformPath};
use alloy_chains::Chain;
use reth_db::{
    init_db,
    mdbx::DatabaseArguments,
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_interfaces::db::LogLevel;
use std::{str::FromStr, sync::Arc};

/// A type that represents either a _real_ (represented by a path), or _test_ database, which will
/// use a [TempDatabase].
#[derive(Debug)]
pub enum DatabaseBuilder {
    /// The real database type, with a specified data dir
    Real(MaybePlatformPath<DataDirPath>),
    /// The test database type
    Test,
}

impl DatabaseBuilder {
    /// Creates a _test_ database
    pub fn test() -> Self {
        Self::Test
    }

    /// Initializes and returns the [DatabaseInstance] depending on the current database type.
    ///
    /// If the [DatabaseBuilder] is test, then the [ChainPath] constructed will be derived from the
    /// db path of the [TempDatabase] and the given chain. The [LogLevel] will not be used.
    ///
    /// If the [DatabaseBuilder] is real, then the db will be initialized using the given log level
    /// and the [ChainPath] will be derived from the given path and chain. This database path is
    /// then passed into [init_db].
    pub fn init_db(
        self,
        log_level: Option<LogLevel>,
        chain: Chain,
    ) -> eyre::Result<DatabaseInstance> {
        match self {
            DatabaseBuilder::Test => {
                let db = create_test_rw_db();
                let db_path_str = db.path().to_str().expect("Path is not valid unicode");
                let path = MaybePlatformPath::<DataDirPath>::from_str(db_path_str)
                    .expect("Path is not valid");
                let data_dir = path.unwrap_or_chain_default(chain);

                Ok(DatabaseInstance::Test { db, data_dir })
            }
            DatabaseBuilder::Real(path) => {
                let data_dir = path.unwrap_or_chain_default(chain);
                let db_path = data_dir.db_path();

                tracing::info!(target: "reth::cli", path = ?db_path, "Opening database");
                let db = Arc::new(
                    init_db(db_path.clone(), DatabaseArguments::default().log_level(log_level))?
                        .with_metrics(),
                );
                Ok(DatabaseInstance::Real { db, data_dir })
            }
        }
    }
}

/// The [Default] implementation for [DatabaseBuilder] uses the _real_ variant, using the default
/// value for the inner [MaybePlatformPath].
impl Default for DatabaseBuilder {
    fn default() -> Self {
        Self::Real(MaybePlatformPath::<DataDirPath>::default())
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_chains::Chain;

    #[test]
    fn test_database_db_dir() {
        // create temp dir to test that the db path is correct
        let tempdir = tempfile::tempdir().unwrap();
        let expected_datadir_path = tempdir.path().to_path_buf();
        let expected_db_path = tempdir.path().join("db");
        let datadir_path = MaybePlatformPath::<DataDirPath>::from(tempdir.path().to_path_buf());
        let db = DatabaseBuilder::Real(datadir_path);
        let db = db.init_db(None, Chain::mainnet()).unwrap();

        // ensure that the datadir path is correct
        assert_eq!(db.data_dir().data_dir_path(), expected_datadir_path);

        // ensure that the db path is correct
        assert_eq!(db.data_dir().db_path(), expected_db_path);
    }
}
