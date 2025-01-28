//! Helper builder entrypoint to instantiate a [`ProviderFactory`].
//!
//! This also includes general purpose staging types that provide builder style functions that lead
//! up to the intended build target.

use crate::{providers::StaticFileProvider, ProviderFactory};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_node_types::{NodeTypes, NodeTypesWithDBAdapter};
use std::{marker::PhantomData, sync::Arc};

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

    // TODO: add helper fns for opening a DB
}

impl<N> Default for ProviderFactoryBuilder<N> {
    fn default() -> Self {
        Self { _types: Default::default() }
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

    // TODO: add helper fns for opening static file provider
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
