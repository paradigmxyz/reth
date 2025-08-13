//! Traits for configuring a node.

use crate::PayloadTypes;
use alloy_rpc_types_engine::JwtSecret;
use reth_basic_payload_builder::PayloadBuilder;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_engine_primitives::{BeaconConsensusEngineEvent, BeaconConsensusEngineHandle};
use reth_evm::ConfigureEvm;
use reth_network_api::FullNetwork;
use reth_node_core::node_config::NodeConfig;
use reth_node_types::{NodeTypes, NodeTypesWithDBAdapter, TxTy};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::FullProvider;
use reth_tasks::TaskExecutor;
use reth_tokio_util::EventSender;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{fmt::Debug, future::Future, marker::PhantomData};

/// A helper trait that is downstream of the [`NodeTypes`] trait and adds stateful
/// components to the node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait FullNodeTypes: Clone + Debug + Send + Sync + Unpin + 'static {
    /// Node's types with the database.
    type Types: NodeTypes;
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + Clone + Unpin + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<NodeTypesWithDBAdapter<Self::Types, Self::DB>>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Clone, Debug)]
pub struct FullNodeTypesAdapter<Types, DB, Provider>(PhantomData<(Types, DB, Provider)>);

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
    Provider: FullProvider<NodeTypesWithDBAdapter<Types, DB>>,
{
    type Types = Types;
    type DB = DB;
    type Provider = Provider;
}

/// Helper trait to bound [`PayloadBuilder`] to the node's engine types.
pub trait PayloadBuilderFor<N: NodeTypes>:
    PayloadBuilder<
    Attributes = <N::Payload as PayloadTypes>::PayloadBuilderAttributes,
    BuiltPayload = <N::Payload as PayloadTypes>::BuiltPayload,
>
{
}

impl<T, N: NodeTypes> PayloadBuilderFor<N> for T where
    T: PayloadBuilder<
        Attributes = <N::Payload as PayloadTypes>::PayloadBuilderAttributes,
        BuiltPayload = <N::Payload as PayloadTypes>::BuiltPayload,
    >
{
}

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + Clone + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Self::Types>>> + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Primitives = <Self::Types as NodeTypes>::Primitives>;

    /// The consensus type of the node.
    type Consensus: FullConsensus<<Self::Types as NodeTypes>::Primitives, Error = ConsensusError>
        + Clone
        + Unpin
        + 'static;

    /// Network API.
    type Network: FullNetwork;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the node's consensus type.
    fn consensus(&self) -> &Self::Consensus;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the handle to the payload builder service handling payload building requests from
    /// the engine.
    fn payload_builder_handle(&self) -> &PayloadBuilderHandle<<Self::Types as NodeTypes>::Payload>;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns an executor handle to spawn tasks.
    ///
    /// This can be used to spawn critical, blocking tasks or register tasks that should be
    /// terminated gracefully. See also [`TaskSpawner`](reth_tasks::TaskSpawner).
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
    pub beacon_engine_handle: BeaconConsensusEngineHandle<<N::Types as NodeTypes>::Payload>,
    /// Notification channel for engine API events
    pub engine_events: EventSender<BeaconConsensusEngineEvent<<N::Types as NodeTypes>::Primitives>>,
    /// JWT secret for the node.
    pub jwt_secret: JwtSecret,
}

/// Customizable node add-on types.
///
/// This trait defines the interface for extending a node with additional functionality beyond
/// the core [`FullNodeComponents`]. It provides a way to launch supplementary services such as
/// RPC servers, monitoring, external integrations, or any custom functionality that builds on
/// top of the core node components.
///
/// ## Purpose
///
/// The `NodeAddOns` trait serves as an extension point in the node builder architecture,
/// allowing developers to:
/// - Define custom services that run alongside the main node
/// - Access all node components and configuration during initialization
/// - Return a handle for managing the launched services (e.g. handle to rpc server)
///
/// ## How it fits into `NodeBuilder`
///
/// In the node builder pattern, add-ons are the final layer that gets applied after all core
/// components are configured and started. The builder flow typically follows:
///
/// 1. Configure [`NodeTypes`] (chain spec, database types, etc.)
/// 2. Build [`FullNodeComponents`] (consensus, networking, transaction pool, etc.)
/// 3. Launch [`NodeAddOns`] with access to all components via [`AddOnsContext`]
///
/// ## Primary Use Case
///
/// The primary use of this trait is to launch RPC servers that provide external API access to
/// the node. For Ethereum nodes, this typically includes two main servers: the regular RPC
/// server (HTTP/WS/IPC) that handles user requests and the authenticated Engine API server
/// that communicates with the consensus layer. The returned handle contains the necessary
/// endpoints and control mechanisms for these servers, allowing the node to serve JSON-RPC
/// requests and participate in consensus. While RPC is the main use case, the trait is
/// intentionally flexible to support other kinds of add-ons such as monitoring, indexing, or
/// custom protocol extensions.
///
/// ## Context Access
///
/// The [`AddOnsContext`] provides access to:
/// - All node components via the `node` field
/// - Node configuration
/// - Engine API handles for consensus layer communication
/// - JWT secrets for authenticated endpoints
///
/// This ensures add-ons can integrate deeply with the node while maintaining clean separation
/// of concerns.
pub trait NodeAddOns<N: FullNodeComponents>: Send {
    /// Handle to add-ons.
    ///
    /// This type is returned by [`launch_add_ons`](Self::launch_add_ons) and represents a
    /// handle to the launched services. It must be `Clone` to allow multiple components to
    /// hold references and should provide methods to interact with the running services.
    ///
    /// For RPC add-ons, this typically includes:
    /// - Server handles to access local addresses and shutdown methods
    /// - RPC module registry for runtime inspection of available methods
    /// - Configured middleware and transport-specific settings
    /// - For Engine API implementations, this also includes handles for consensus layer
    ///   communication
    type Handle: Send + Sync + Clone;

    /// Configures and launches the add-ons.
    ///
    /// This method is called once during node startup after all core components are initialized.
    /// It receives an [`AddOnsContext`] that provides access to:
    ///
    /// - The fully configured node with all its components
    /// - Node configuration for reading settings
    /// - Engine API handles for consensus layer communication
    /// - JWT secrets for setting up authenticated endpoints (if any).
    ///
    /// The implementation should:
    /// 1. Use the context to configure the add-on services
    /// 2. Launch any background tasks using the node's task executor
    /// 3. Return a handle that allows interaction with the launched services
    ///
    /// # Errors
    ///
    /// This method may fail if the add-ons cannot be properly configured or launched,
    /// for example due to port binding issues or invalid configuration.
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
