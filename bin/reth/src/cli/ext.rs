//! Support for integrating customizations into the CLI.

use crate::cli::config::{PayloadBuilderConfig, RethRpcConfig};
use clap::Args;
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_network_api::{NetworkInfo, Peers};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_primitives::ChainSpec;
use reth_provider::{
    BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, EvmEnvProvider,
    StateProviderFactory,
};
use reth_rpc_builder::{RethModuleRegistry, TransportRpcModules};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{fmt, sync::Arc};

/// A trait that allows for extending parts of the CLI with additional functionality.
///
/// This is intended as a way to allow to _extend_ the node command. For example, to register
/// additional RPC namespaces.
pub trait RethCliExt {
    /// Provides additional configuration for the node CLI command.
    ///
    /// This supports additional CLI arguments that can be used to modify the node configuration.
    ///
    /// If no additional CLI arguments are required, the [NoArgs] wrapper type can be used.
    type Node: RethNodeCommandExt;
}

/// The default CLI extension.
impl RethCliExt for () {
    type Node = DefaultRethNodeCommandConfig;
}

/// A trait that allows for extending and customizing parts of the node command
/// [NodeCommand](crate::node::NodeCommand).
pub trait RethNodeCommandConfig: fmt::Debug {
    /// Allows for registering additional RPC modules for the transports.
    ///
    /// This is expected to call the merge functions of [TransportRpcModules], for example
    /// [TransportRpcModules::merge_configured]
    fn extend_rpc_modules<Conf, Provider, Pool, Network, Tasks, Events>(
        &mut self,
        _config: &Conf,
        _registry: &mut RethModuleRegistry<Provider, Pool, Network, Tasks, Events>,
        _modules: &mut TransportRpcModules,
    ) -> eyre::Result<()>
    where
        Conf: RethRpcConfig,
        Provider: BlockReaderIdExt
            + StateProviderFactory
            + EvmEnvProvider
            + ChainSpecProvider
            + ChangeSetReader
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        Events: CanonStateSubscriptions + Clone + 'static,
    {
        Ok(())
    }

    /// Configures the [PayloadBuilderService] for the node, spawns it and returns the
    /// [PayloadBuilderHandle].
    ///
    /// By default this spawns a [BasicPayloadJobGenerator] with the default configuration
    /// [BasicPayloadJobGeneratorConfig].
    fn spawn_payload_builder_service<Conf, Provider, Pool, Tasks>(
        &mut self,
        conf: &Conf,
        provider: Provider,
        pool: Pool,
        executor: Tasks,
        chain_spec: Arc<ChainSpec>,
    ) -> eyre::Result<PayloadBuilderHandle>
    where
        Conf: PayloadBuilderConfig,
        Provider: StateProviderFactory + BlockReaderIdExt + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
        Tasks: TaskSpawner + Clone + Unpin + 'static,
    {
        let payload_generator = BasicPayloadJobGenerator::new(
            provider,
            pool,
            executor.clone(),
            BasicPayloadJobGeneratorConfig::default()
                .interval(conf.interval())
                .deadline(conf.deadline())
                .max_payload_tasks(conf.max_payload_tasks())
                .extradata(conf.extradata_rlp_bytes())
                .max_gas_limit(conf.max_gas_limit()),
            chain_spec,
        );
        let (payload_service, payload_builder) = PayloadBuilderService::new(payload_generator);

        executor.spawn_critical("payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
    }
}

/// A trait that allows for extending parts of the CLI with additional functionality.
pub trait RethNodeCommandExt: RethNodeCommandConfig + fmt::Debug + clap::Args {}

// blanket impl for all types that implement the required traits.
impl<T> RethNodeCommandExt for T where T: RethNodeCommandConfig + fmt::Debug + clap::Args {}

/// The default configuration for the reth node command [Command](crate::node::NodeCommand).
///
/// This is a convenience type for [NoArgs<()>].
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct DefaultRethNodeCommandConfig;

impl RethNodeCommandConfig for DefaultRethNodeCommandConfig {}

impl RethNodeCommandConfig for () {}

/// A helper struct that allows for wrapping a [RethNodeCommandConfig] value without providing
/// additional CLI arguments.
///
/// Note: This type must be manually filled with a [RethNodeCommandConfig] manually before executing
/// the [NodeCommand](crate::node::NodeCommand).
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct NoArgs<T> {
    #[clap(skip)]
    inner: Option<T>,
}

impl<T> NoArgs<T> {
    /// Creates a new instance of the wrapper type.
    pub fn with(inner: T) -> Self {
        Self { inner: Some(inner) }
    }

    /// Sets the inner value.
    pub fn set(&mut self, inner: T) {
        self.inner = Some(inner)
    }

    /// Transforms the configured value.
    pub fn map<U>(self, inner: U) -> NoArgs<U> {
        NoArgs::with(inner)
    }

    /// Returns the inner value if it exists.
    pub fn inner(&self) -> Option<&T> {
        self.inner.as_ref()
    }

    /// Returns a mutable reference to the inner value if it exists.
    pub fn inner_mut(&mut self) -> Option<&mut T> {
        self.inner.as_mut()
    }

    /// Consumes the wrapper and returns the inner value if it exists.
    pub fn into_inner(self) -> Option<T> {
        self.inner
    }
}

impl<T: RethNodeCommandConfig> RethNodeCommandConfig for NoArgs<T> {
    fn extend_rpc_modules<Conf, Provider, Pool, Network, Tasks, Events>(
        &mut self,
        config: &Conf,
        registry: &mut RethModuleRegistry<Provider, Pool, Network, Tasks, Events>,
        modules: &mut TransportRpcModules<()>,
    ) -> eyre::Result<()>
    where
        Conf: RethRpcConfig,
        Provider: BlockReaderIdExt
            + StateProviderFactory
            + EvmEnvProvider
            + ChainSpecProvider
            + ChangeSetReader
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        Events: CanonStateSubscriptions + Clone + 'static,
    {
        if let Some(conf) = self.inner_mut() {
            conf.extend_rpc_modules(config, registry, modules)
        } else {
            Ok(())
        }
    }

    fn spawn_payload_builder_service<Conf, Provider, Pool, Tasks>(
        &mut self,
        conf: &Conf,
        provider: Provider,
        pool: Pool,
        executor: Tasks,
        chain_spec: Arc<ChainSpec>,
    ) -> eyre::Result<PayloadBuilderHandle>
    where
        Conf: PayloadBuilderConfig,
        Provider: StateProviderFactory + BlockReaderIdExt + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
        Tasks: TaskSpawner + Clone + Unpin + 'static,
    {
        self.inner_mut()
            .ok_or_else(|| eyre::eyre!("config value must be set"))?
            .spawn_payload_builder_service(conf, provider, pool, executor, chain_spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_ext<T: RethNodeCommandExt>() {}

    #[test]
    fn ensure_ext() {
        assert_ext::<DefaultRethNodeCommandConfig>();
        assert_ext::<NoArgs<()>>();
    }
}
