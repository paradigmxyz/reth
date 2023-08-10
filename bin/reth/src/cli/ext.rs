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
    type Node: RethNodeCommandExt;
}

/// The default CLI extension.
impl RethCliExt for () {
    type Node = DefaultRethNodeCommandConfig;
}

/// A trait that allows for extending parts of the CLI with additional functionality.
pub trait RethNodeCommandExt: fmt::Debug + clap::Args {
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

    // TODO move network related functions here
}

/// The default configuration for the reth node command [Command](crate::node::NodeCommand).
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct DefaultRethNodeCommandConfig;

impl RethNodeCommandExt for DefaultRethNodeCommandConfig {}
