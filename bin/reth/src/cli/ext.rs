//! Support for integrating customizations into the CLI.

use clap::Args;
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{
    BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, EvmEnvProvider,
    StateProviderFactory,
};
use reth_rpc_builder::{EthConfig, RethModuleRegistry, TransportRpcModules};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::fmt;

/// A trait that allows for extending parts of the CLI with additional functionality.
pub trait RethCliExt {
    /// Extends the rpc arguments for the node
    type RpcExt: RethRpcServerArgsExt;
}

impl RethCliExt for () {
    type RpcExt = NoopArgsExt;
}

/// An [Args] extension that does nothing.
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct NoopArgsExt;

/// A trait that provides configured RPC server.
///
/// This provides all basic config values for the RPC server and is implemented by the
/// [RpcServerArgs](crate::args::RpcServerArgs) type.
pub trait RethRpcConfig {
    /// Returns whether ipc is enabled.
    fn is_ipc_enabled(&self) -> bool;

    /// The configured ethereum RPC settings.
    fn eth_config(&self) -> EthConfig;

    // TODO extract more functions from RpcServerArgs
}

/// A trait that allows further customization of the RPC server via CLI.
pub trait RethRpcServerArgsExt: fmt::Debug + clap::Args {
    /// Allows for registering additional RPC modules for the transports.
    ///
    /// This is expected to call the merge functions of [TransportRpcModules], for example
    /// [TransportRpcModules::merge_configured]
    fn extend_rpc_modules<Conf, Provider, Pool, Network, Tasks, Events>(
        &self,
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
        Events: CanonStateSubscriptions + Clone + 'static;
}

impl RethRpcServerArgsExt for NoopArgsExt {
    fn extend_rpc_modules<Conf, Provider, Pool, Network, Tasks, Events>(
        &self,
        _config: &Conf,
        _registry: &mut RethModuleRegistry<Provider, Pool, Network, Tasks, Events>,
        _modules: &mut TransportRpcModules<()>,
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
}
