//! Helper builder entrypoint to instantiate a [`ProviderFactory`].
//!
//! This also includes general purpose staging types that provide builder style functions that lead
//! up to the intended build target.

use crate::{providers::StaticFileProvider, ProviderFactory};
use reth_db::{open_db_read_only, DatabaseArguments, DatabaseEnv};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_node_types::{NodeTypes, NodeTypesWithDBAdapter};
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
    pub fn db<DB>(self, db: DB) -> TypesAnd1<N, DB> {
        TypesAnd1::new(db)
    }

    /// Opens the database with the given chainspec and [`ReadOnlyConfig`].
    ///
    /// # Open a monitored instance
    ///
    /// This is recommended when the new read-only instance is used with an active node.
    ///
    /// ```no_run
    /// use reth_chainspec::MAINNET;
    /// use reth_node_types::NodeTypes;
    /// use reth_provider::providers::ProviderFactoryBuilder;
    ///
    /// fn demo<N: NodeTypes<ChainSpec = reth_chainspec::ChainSpec>>() {
    ///     let provider_factory = ProviderFactoryBuilder::<N>::default()
    ///         .open_read_only(MAINNET.clone(), "datadir")
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
    /// use reth_node_types::NodeTypes;
    ///
    /// use reth_provider::providers::{ProviderFactoryBuilder, ReadOnlyConfig};
    ///
    /// fn demo<N: NodeTypes<ChainSpec = reth_chainspec::ChainSpec>>() {
    ///     let provider_factory = ProviderFactoryBuilder::<N>::default()
    ///         .open_read_only(MAINNET.clone(), ReadOnlyConfig::from_datadir("datadir").no_watch())
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
    /// use reth_node_types::NodeTypes;
    ///
    /// use reth_provider::providers::{ProviderFactoryBuilder, ReadOnlyConfig};
    ///
    /// fn demo<N: NodeTypes<ChainSpec = reth_chainspec::ChainSpec>>() {
    ///     let provider_factory = ProviderFactoryBuilder::<N>::default()
    ///         .open_read_only(
    ///             MAINNET.clone(),
    ///             ReadOnlyConfig::from_datadir("datadir").disable_long_read_transaction_safety(),
    ///         )
    ///         .unwrap();
    /// }
    /// ```
    pub fn open_read_only(
        self,
        chainspec: Arc<N::ChainSpec>,
        config: impl Into<ReadOnlyConfig>,
    ) -> eyre::Result<ProviderFactory<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>>
    where
        N: NodeTypes,
    {
        let ReadOnlyConfig { db_dir, db_args, static_files_dir, watch_static_files } =
            config.into();
        Ok(self
            .db(Arc::new(open_db_read_only(db_dir, db_args)?))
            .chainspec(chainspec)
            .static_file(StaticFileProvider::read_only(static_files_dir, watch_static_files)?)
            .build_provider_factory())
    }
}

impl<N> Default for ProviderFactoryBuilder<N> {
    fn default() -> Self {
        Self { _types: Default::default() }
    }
}

/// Settings for how to open the database and static files.
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
    ///    |__static_files
    /// ```
    ///
    /// By default this watches the static file directory for changes, see also
    /// [`StaticFileProvider::read_only`]
    pub fn from_datadir(datadir: impl AsRef<Path>) -> Self {
        let datadir = datadir.as_ref();
        Self::from_dirs(datadir.join("db"), datadir.join("static_files"))
    }

    /// Disables long-lived read transaction safety guarantees.
    ///
    /// Caution: Keeping database transaction open indefinitely can cause the free list to grow if
    /// changes to the database are made.
    pub fn disable_long_read_transaction_safety(mut self) -> Self {
        self.db_args = self.db_args.with_max_read_transaction_duration_unbounded();
        self
    }

    /// Derives the [`ReadOnlyConfig`] from the database dir.
    ///
    /// By default this assumes the following datadir layout:
    ///
    /// ```text
    ///    - db
    ///    -static_files
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
        let static_files_dir = std::fs::canonicalize(db_dir)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
            .join("static_files");
        Self::from_dirs(db_dir, static_files_dir)
    }

    /// Creates the config for the given paths.
    ///
    ///
    /// By default this watches the static file directory for changes, see also
    /// [`StaticFileProvider::read_only`]
    pub fn from_dirs(db_dir: impl AsRef<Path>, static_files_dir: impl AsRef<Path>) -> Self {
        Self {
            static_files_dir: static_files_dir.as_ref().into(),
            db_dir: db_dir.as_ref().into(),
            db_args: Default::default(),
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

/// This is staging type that contains the configured types and _one_ value.
#[derive(Debug)]
pub struct TypesAnd1<N, Val1> {
    _types: PhantomData<N>,
    val_1: Val1,
}

impl<N, Val1> TypesAnd1<N, Val1> {
    /// Creates a new instance with the given types and one value.
    pub fn new(val_1: Val1) -> Self {
        Self { _types: Default::default(), val_1 }
    }

    /// Configures the chainspec.
    pub fn chainspec<C>(self, chainspec: Arc<C>) -> TypesAnd2<N, Val1, Arc<C>> {
        TypesAnd2::new(self.val_1, chainspec)
    }
}

/// This is staging type that contains the configured types and _two_ values.
#[derive(Debug)]
pub struct TypesAnd2<N, Val1, Val2> {
    _types: PhantomData<N>,
    val_1: Val1,
    val_2: Val2,
}

impl<N, Val1, Val2> TypesAnd2<N, Val1, Val2> {
    /// Creates a new instance with the given types and two values.
    pub fn new(val_1: Val1, val_2: Val2) -> Self {
        Self { _types: Default::default(), val_1, val_2 }
    }

    /// Returns the first value.
    pub const fn val_1(&self) -> &Val1 {
        &self.val_1
    }

    /// Returns the second value.
    pub const fn val_2(&self) -> &Val2 {
        &self.val_2
    }

    /// Configures the [`StaticFileProvider`].
    pub fn static_file(
        self,
        static_file_provider: StaticFileProvider<N::Primitives>,
    ) -> TypesAnd3<N, Val1, Val2, StaticFileProvider<N::Primitives>>
    where
        N: NodeTypes,
    {
        TypesAnd3::new(self.val_1, self.val_2, static_file_provider)
    }
}

/// This is staging type that contains the configured types and _three_ values.
#[derive(Debug)]
pub struct TypesAnd3<N, Val1, Val2, Val3> {
    _types: PhantomData<N>,
    val_1: Val1,
    val_2: Val2,
    val_3: Val3,
}

impl<N, Val1, Val2, Val3> TypesAnd3<N, Val1, Val2, Val3> {
    /// Creates a new instance with the given types and three values.
    pub fn new(val_1: Val1, val_2: Val2, val_3: Val3) -> Self {
        Self { _types: Default::default(), val_1, val_2, val_3 }
    }
}

impl<N, DB> TypesAnd3<N, DB, Arc<N::ChainSpec>, StaticFileProvider<N::Primitives>>
where
    N: NodeTypes,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
{
    /// Creates the [`ProviderFactory`].
    pub fn build_provider_factory(self) -> ProviderFactory<NodeTypesWithDBAdapter<N, DB>> {
        let Self { _types, val_1, val_2, val_3 } = self;
        ProviderFactory::new(val_1, val_2, val_3)
    }
}
