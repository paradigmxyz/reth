use futures::Future;
use reth_cli::chainspec::ChainSpecParser;
use reth_db::DatabaseEnv;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use std::{fmt, sync::Arc};

/// A trait for launching a reth node with custom configuration strategies.
///
/// This trait allows defining node configuration through various object types rather than just
/// functions. By implementing this trait on your own structures, you can:
///
/// - Create flexible configurations that connect necessary components without creating separate
///   closures
/// - Take advantage of decomposition to break complex configurations into a series of methods
/// - Encapsulate configuration logic in dedicated types with their own state and behavior
/// - Reuse configuration patterns across different parts of your application
pub trait Launcher<C, Ext>
where
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
{
    /// Entry point for launching a node with custom configuration.
    ///
    /// Consumes `self` to use pre-configured state, takes a builder and arguments,
    /// and returns an async future.
    ///
    /// # Arguments
    ///
    /// * `builder` - Node builder with launch context
    /// * `builder_args` - Extension arguments for configuration
    fn entrypoint(
        self,
        builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
        builder_args: Ext,
    ) -> impl Future<Output = eyre::Result<()>>;
}

/// A function-based adapter implementation of the [`Launcher`] trait.
///
/// This struct adapts existing closures to work with the new [`Launcher`] trait,
/// maintaining backward compatibility with current node implementations while
/// enabling the transition to the more flexible trait-based approach.
pub struct FnLauncher<F> {
    /// The function to execute when launching the node
    func: F,
}

impl<F> FnLauncher<F> {
    /// Creates a new function launcher adapter.
    ///
    /// Type parameters `C` and `Ext` help the compiler infer correct types
    /// since they're not stored in the struct itself.
    ///
    /// # Arguments
    ///
    /// * `func` - Function that configures and launches a node
    pub fn new<C, Ext>(func: F) -> Self
    where
        C: ChainSpecParser,
        F: AsyncFnOnce(
            WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
            Ext,
        ) -> eyre::Result<()>,
    {
        Self { func }
    }
}

impl<C, Ext, F> Launcher<C, Ext> for FnLauncher<F>
where
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
    F: AsyncFnOnce(
        WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
        Ext,
    ) -> eyre::Result<()>,
{
    fn entrypoint(
        self,
        builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
        builder_args: Ext,
    ) -> impl Future<Output = eyre::Result<()>> {
        (self.func)(builder, builder_args)
    }
}
