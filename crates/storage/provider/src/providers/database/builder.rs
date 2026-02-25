//! Helper builder entrypoint to instantiate a [`ProviderFactory`].

use crate::{
    providers::{NodeTypesForProvider, RocksDBProvider, StaticFileProvider},
    ProviderFactory,
};
use reth_db::{
    mdbx::{DatabaseArguments, MaxReadTransactionDuration},
    open_db_read_only, DatabaseEnv,
};
use reth_node_types::NodeTypesWithDBAdapter;
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Helper type to create a [`ProviderFactory`].
///
/// See [`ProviderFactoryBuilder::open_read_only`] for usage examples.
#[derive(Debug)]
pub struct ProviderFactoryBuilder<N> {
    _types: PhantomData<N>,
}

impl<N> ProviderFactoryBuilder<N> {
    /// Maps the [`reth_node_types::NodeTypes`] of this builder.
    pub fn types<T>(self) -> ProviderFactoryBuilder<T> {
        ProviderFactoryBuilder::default()
    }

    /// Opens the database with the given chainspec and [`ReadOnlyConfig`].
    ///
    /// # Open a monitored instance
    ///
    /// This is recommended when the new read-only instance is used with an active node.
    ///
    /// ```no_run
    /// use reth_chainspec::MAINNET;
    /// use reth_provider::providers::{NodeTypesForProvider, ProviderFactoryBuilder};
    ///
    /// fn demo<N: NodeTypesForProvider<ChainSpec = reth_chainspec::ChainSpec>>(
    ///     runtime: reth_tasks::Runtime,
    /// ) {
    ///     let provider_factory = ProviderFactoryBuilder::<N>::default()
    ///         .open_read_only(MAINNET.clone(), "datadir", runtime)
    ///         .unwrap();
    /// }
    /// ```
    ///
    /// # Open an unmonitored instance
    ///
    /// This is recommended when no changes to the static files are expected (e.g. no active node)
    ///
    /// ```no_run
    /// use reth_chainspec::MAINNET;
    /// use reth_provider::providers::{NodeTypesForProvider, ProviderFactoryBuilder, ReadOnlyConfig};
    ///
    /// fn demo<N: NodeTypesForProvider<ChainSpec = reth_chainspec::ChainSpec>>(
    ///     runtime: reth_tasks::Runtime,
    /// ) {
    ///     let provider_factory = ProviderFactoryBuilder::<N>::default()
    ///         .open_read_only(
    ///             MAINNET.clone(),
    ///             ReadOnlyConfig::from_datadir("datadir").no_watch(),
    ///             runtime,
    ///         )
    ///         .unwrap();
    /// }
    /// ```
    ///
    /// # Open an instance with disabled read-transaction timeout
    ///
    /// By default, read transactions are automatically terminated after a timeout to prevent
    /// database free list growth. However, if the database is static (no writes occurring), this
    /// safety mechanism can be disabled using
    /// [`ReadOnlyConfig::disable_long_read_transaction_safety`].
    ///
    /// ```no_run
    /// use reth_chainspec::MAINNET;
    /// use reth_provider::providers::{NodeTypesForProvider, ProviderFactoryBuilder, ReadOnlyConfig};
    ///
    /// fn demo<N: NodeTypesForProvider<ChainSpec = reth_chainspec::ChainSpec>>(
    ///     runtime: reth_tasks::Runtime,
    /// ) {
    ///     let provider_factory = ProviderFactoryBuilder::<N>::default()
    ///         .open_read_only(
    ///             MAINNET.clone(),
    ///             ReadOnlyConfig::from_datadir("datadir").disable_long_read_transaction_safety(),
    ///             runtime,
    ///         )
    ///         .unwrap();
    /// }
    /// ```
    pub fn open_read_only(
        self,
        chainspec: Arc<N::ChainSpec>,
        config: impl Into<ReadOnlyConfig>,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<ProviderFactory<NodeTypesWithDBAdapter<N, DatabaseEnv>>>
    where
        N: NodeTypesForProvider,
    {
        let ReadOnlyConfig { db_dir, db_args, static_files_dir, rocksdb_dir, watch_static_files } =
            config.into();
        let db = open_db_read_only(db_dir, db_args)?;
        let static_file_provider =
            StaticFileProvider::read_only(static_files_dir, watch_static_files)?;
        let rocksdb_provider =
            RocksDBProvider::builder(&rocksdb_dir).with_default_tables().build()?;
        ProviderFactory::new(db, chainspec, static_file_provider, rocksdb_provider, runtime)
            .map_err(Into::into)
    }
}

impl<N> Default for ProviderFactoryBuilder<N> {
    fn default() -> Self {
        Self { _types: Default::default() }
    }
}

/// Settings for how to open the database, static files, and `RocksDB`.
///
/// The default derivation from a path assumes the path is the datadir:
/// [`ReadOnlyConfig::from_datadir`]
#[derive(Debug, Clone)]
pub struct ReadOnlyConfig {
    /// The path to the database directory.
    pub db_dir: PathBuf,
    /// How to open the database
    pub db_args: DatabaseArguments,
    /// The path to the static file dir
    pub static_files_dir: PathBuf,
    /// The path to the `RocksDB` directory
    pub rocksdb_dir: PathBuf,
    /// Whether the static files should be watched for changes.
    pub watch_static_files: bool,
}

impl ReadOnlyConfig {
    /// Derives the [`ReadOnlyConfig`] from the datadir.
    ///
    /// By default this assumes the following datadir layout:
    ///
    /// ```text
    ///  -`datadir`
    ///    |__db
    ///    |__rocksdb
    ///    |__static_files
    /// ```
    ///
    /// By default this watches the static file directory for changes, see also
    /// [`StaticFileProvider::read_only`]
    pub fn from_datadir(datadir: impl AsRef<Path>) -> Self {
        let datadir = datadir.as_ref();
        Self {
            db_dir: datadir.join("db"),
            db_args: Default::default(),
            static_files_dir: datadir.join("static_files"),
            rocksdb_dir: datadir.join("rocksdb"),
            watch_static_files: true,
        }
    }

    /// Disables long-lived read transaction safety guarantees.
    ///
    /// Caution: Keeping database transaction open indefinitely can cause the free list to grow if
    /// changes to the database are made.
    pub const fn disable_long_read_transaction_safety(mut self) -> Self {
        self.db_args.max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded));
        self
    }

    /// Derives the [`ReadOnlyConfig`] from the database dir.
    ///
    /// By default this assumes the following datadir layout:
    ///
    /// ```text
    ///    - db
    ///    - rocksdb
    ///    - static_files
    /// ```
    ///
    /// By default this watches the static file directory for changes, see also
    /// [`StaticFileProvider::read_only`]
    ///
    /// # Panics
    ///
    /// If the path does not exist
    pub fn from_db_dir(db_dir: impl AsRef<Path>) -> Self {
        let db_dir = db_dir.as_ref();
        let datadir = std::fs::canonicalize(db_dir).unwrap().parent().unwrap().to_path_buf();
        let static_files_dir = datadir.join("static_files");
        let rocksdb_dir = datadir.join("rocksdb");
        Self::from_dirs(db_dir, static_files_dir, rocksdb_dir)
    }

    /// Creates the config for the given paths.
    ///
    ///
    /// By default this watches the static file directory for changes, see also
    /// [`StaticFileProvider::read_only`]
    pub fn from_dirs(
        db_dir: impl AsRef<Path>,
        static_files_dir: impl AsRef<Path>,
        rocksdb_dir: impl AsRef<Path>,
    ) -> Self {
        Self {
            db_dir: db_dir.as_ref().into(),
            db_args: Default::default(),
            static_files_dir: static_files_dir.as_ref().into(),
            rocksdb_dir: rocksdb_dir.as_ref().into(),
            watch_static_files: true,
        }
    }

    /// Configures the db arguments used when opening the database.
    pub fn with_db_args(mut self, db_args: impl Into<DatabaseArguments>) -> Self {
        self.db_args = db_args.into();
        self
    }

    /// Configures the db directory.
    pub fn with_db_dir(mut self, db_dir: impl Into<PathBuf>) -> Self {
        self.db_dir = db_dir.into();
        self
    }

    /// Configures the static file directory.
    pub fn with_static_file_dir(mut self, static_file_dir: impl Into<PathBuf>) -> Self {
        self.static_files_dir = static_file_dir.into();
        self
    }

    /// Whether the static file directory should be watches for changes, see also
    /// [`StaticFileProvider::read_only`]
    pub const fn set_watch_static_files(&mut self, watch_static_files: bool) {
        self.watch_static_files = watch_static_files;
    }

    /// Don't watch the static files for changes.
    ///
    /// This is only recommended if this is used without a running node instance that modifies
    /// static files.
    pub const fn no_watch(mut self) -> Self {
        self.set_watch_static_files(false);
        self
    }
}

impl<T> From<T> for ReadOnlyConfig
where
    T: AsRef<Path>,
{
    fn from(value: T) -> Self {
        Self::from_datadir(value.as_ref())
    }
}
