//! Helper builder entrypoint to instantiate a [`ProviderFactory`].
//!
//! This also includes general purpose staging types that provide builder style functions that lead
//! up to the intended build target.

use crate::{
    providers::{NodeTypesForProvider, RocksDBProvider, StaticFileProvider},
    ProviderFactory,
};
use reth_db::{
    mdbx::{DatabaseArguments, MaxReadTransactionDuration},
    open_db_read_only, DatabaseEnv,
};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_node_types::{NodeTypes, NodeTypesWithDBAdapter};
use reth_storage_errors::provider::ProviderResult;
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Helper type to create a [`ProviderFactory`].
///
/// This type is the entry point for a stage based builder.
///
/// The intended staging is:
///  1. Configure the database: [`ProviderFactoryBuilder::db`]
///  2. Configure the chainspec: `chainspec`
///  3. Configure the [`StaticFileProvider`]: `static_file`
#[derive(Debug)]
pub struct ProviderFactoryBuilder<N> {
    _types: PhantomData<N>,
}

impl<N> ProviderFactoryBuilder<N> {
    /// Maps the [`NodeTypes`] of this builder.
    pub fn types<T>(self) -> ProviderFactoryBuilder<T> {
        ProviderFactoryBuilder::default()
    }

    /// Configures the database.
    pub fn db<DB>(self, db: DB) -> WithDb<N, DB> {
        WithDb::new(db)
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
        self.db(open_db_read_only(db_dir, db_args)?)
            .chainspec(chainspec)
            .static_file(StaticFileProvider::read_only(static_files_dir, watch_static_files)?)
            .rocksdb_provider(RocksDBProvider::builder(&rocksdb_dir).with_default_tables().build()?)
            .runtime(runtime)
            .build_provider_factory()
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

/// Builder stage after configuring the database.
#[derive(Debug)]
pub struct WithDb<N, DB> {
    _types: PhantomData<N>,
    db: DB,
}

impl<N, DB> WithDb<N, DB> {
    /// Creates a new instance with the given database.
    pub fn new(db: DB) -> Self {
        Self { _types: Default::default(), db }
    }

    /// Configures the chainspec.
    pub fn chainspec<C>(self, chainspec: Arc<C>) -> WithChainSpec<N, DB, C> {
        WithChainSpec { _types: Default::default(), db: self.db, chainspec }
    }
}

/// Builder stage after configuring the database and chainspec.
#[derive(Debug)]
pub struct WithChainSpec<N, DB, C> {
    _types: PhantomData<N>,
    db: DB,
    chainspec: Arc<C>,
}

impl<N, DB, C> WithChainSpec<N, DB, C> {
    /// Returns the database.
    pub const fn db(&self) -> &DB {
        &self.db
    }

    /// Returns the chainspec.
    pub const fn chainspec(&self) -> &Arc<C> {
        &self.chainspec
    }

    /// Configures the [`StaticFileProvider`].
    pub fn static_file(
        self,
        static_file_provider: StaticFileProvider<N::Primitives>,
    ) -> WithStaticFiles<N, DB, C>
    where
        N: NodeTypes,
    {
        WithStaticFiles {
            _types: Default::default(),
            db: self.db,
            chainspec: self.chainspec,
            static_file_provider,
        }
    }
}

/// Builder stage after configuring the database, chainspec, and static file provider.
#[derive(Debug)]
pub struct WithStaticFiles<N: NodeTypes, DB, C> {
    _types: PhantomData<N>,
    db: DB,
    chainspec: Arc<C>,
    static_file_provider: StaticFileProvider<N::Primitives>,
}

impl<N: NodeTypes, DB, C> WithStaticFiles<N, DB, C> {
    /// Configures the `RocksDB` provider.
    pub fn rocksdb_provider(self, rocksdb_provider: RocksDBProvider) -> WithRocksDb<N, DB, C> {
        WithRocksDb {
            _types: Default::default(),
            db: self.db,
            chainspec: self.chainspec,
            static_file_provider: self.static_file_provider,
            rocksdb_provider,
        }
    }
}

/// Builder stage after configuring the database, chainspec, static file provider, and `RocksDB`.
#[derive(Debug)]
pub struct WithRocksDb<N: NodeTypes, DB, C> {
    _types: PhantomData<N>,
    db: DB,
    chainspec: Arc<C>,
    static_file_provider: StaticFileProvider<N::Primitives>,
    rocksdb_provider: RocksDBProvider,
}

impl<N, DB> WithRocksDb<N, DB, N::ChainSpec>
where
    N: NodeTypesForProvider,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
{
    /// Sets the task runtime for the provider factory.
    pub fn runtime(self, runtime: reth_tasks::Runtime) -> WithRuntime<N, DB> {
        WithRuntime {
            _types: Default::default(),
            db: self.db,
            chainspec: self.chainspec,
            static_file_provider: self.static_file_provider,
            rocksdb_provider: self.rocksdb_provider,
            runtime,
        }
    }
}

/// Final builder stage with all components configured, ready to build a [`ProviderFactory`].
#[derive(Debug)]
pub struct WithRuntime<N: NodeTypes, DB> {
    _types: PhantomData<N>,
    db: DB,
    chainspec: Arc<N::ChainSpec>,
    static_file_provider: StaticFileProvider<N::Primitives>,
    rocksdb_provider: RocksDBProvider,
    runtime: reth_tasks::Runtime,
}

impl<N, DB> WithRuntime<N, DB>
where
    N: NodeTypesForProvider,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
{
    /// Creates the [`ProviderFactory`].
    pub fn build_provider_factory(
        self,
    ) -> ProviderResult<ProviderFactory<NodeTypesWithDBAdapter<N, DB>>> {
        ProviderFactory::new(
            self.db,
            self.chainspec,
            self.static_file_provider,
            self.rocksdb_provider,
            self.runtime,
        )
    }
}
