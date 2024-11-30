//! Traits for configuring a node.

use crate::ConfigureEvm;
use alloy_consensus::Header;
use alloy_rpc_types_engine::JwtSecret;
use reth_beacon_consensus::BeaconConsensusEngineHandle;
use reth_consensus::FullConsensus;
use reth_evm::execute::BlockExecutorProvider;
use reth_network_api::FullNetwork;
use reth_node_core::node_config::NodeConfig;
use reth_node_types::{NodeTypes, NodeTypesWithDB, NodeTypesWithEngine};
use reth_payload_builder_primitives::PayloadBuilder;
use reth_provider::FullProvider;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;
use std::{future::Future, marker::PhantomData};

/// A helper trait that is downstream of the [`NodeTypesWithEngine`] trait and adds stateful
/// components to the node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait FullNodeTypes: Send + Sync + Unpin + 'static {
    /// Node's types with the database.
    type Types: NodeTypesWithDB + NodeTypesWithEngine;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<Self::Types>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, Provider> {
    /// An instance of the user configured node types.
    pub types: PhantomData<Types>,
    /// The provider type used by the node.
    pub provider: PhantomData<Provider>,
}

impl<Types, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, Provider>
where
    Types: NodeTypesWithDB + NodeTypesWithEngine,
    Provider: FullProvider<Types>,
{
    type Types = Types;
    type Provider = Provider;
}

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + Clone + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Header = Header>;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider<Primitives = <Self::Types as NodeTypes>::Primitives>;

    /// The consensus type of the node.
    type Consensus: FullConsensus<<Self::Types as NodeTypes>::Primitives> + Clone + Unpin + 'static;

    /// Network API.
    type Network: FullNetwork;

    /// Builds new blocks.
    type PayloadBuilder: PayloadBuilder + Clone;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the node's executor type.
    fn block_executor(&self) -> &Self::Executor;

    /// Returns the node's consensus type.
    fn consensus(&self) -> &Self::Consensus;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &Self::PayloadBuilder;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns handle to runtime.
    fn task_executor(&self) -> &TaskExecutor;
}

/// Context passed to [`NodeAddOns::launch_add_ons`],
#[derive(Debug, Clone)]
pub struct AddOnsContext<'a, N: FullNodeComponents> {
    /// Node with all configured components.
    pub node: N,
    /// Node configuration.
    pub config: &'a NodeConfig<<N::Types as NodeTypes>::ChainSpec>,
    /// Handle to the beacon consensus engine.
    pub beacon_engine_handle:
        BeaconConsensusEngineHandle<<N::Types as NodeTypesWithEngine>::Engine>,
    /// JWT secret for the node.
    pub jwt_secret: JwtSecret,
}

/// Customizable node add-on types.
pub trait NodeAddOns<N: FullNodeComponents>: Send {
    /// Handle to add-ons.
    type Handle: Send + Sync + Clone;

    /// Configures and launches the add-ons.
    fn launch_add_ons(
        self,
        ctx: AddOnsContext<'_, N>,
    ) -> impl Future<Output = eyre::Result<Self::Handle>> + Send;
}

impl<N: FullNodeComponents> NodeAddOns<N> for () {
    type Handle = ();

    async fn launch_add_ons(self, _components: AddOnsContext<'_, N>) -> eyre::Result<Self::Handle> {
        Ok(())
    }
}
