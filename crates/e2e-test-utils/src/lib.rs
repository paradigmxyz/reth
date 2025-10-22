//! Utilities for end-to-end tests.

use node::NodeTestContext;
use reth_chainspec::ChainSpec;
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_builder::{
    components::NodeComponentsBuilder,
    rpc::{EngineValidatorAddOn, RethRpcAddOns},
    FullNodeTypesAdapter, Node, NodeAdapter, NodeComponents, NodePrimitives, NodeTypes,
    NodeTypesWithDBAdapter, PayloadAttributesBuilder, PayloadTypes,
};
use reth_provider::providers::{BlockchainProvider, NodeTypesForProvider};
use reth_tasks::TaskManager;
use std::sync::Arc;
use wallet::Wallet;

/// Wrapper type to create test nodes
pub mod node;
pub mod testsuite;

/// Helper for transaction operations
pub mod transaction;

/// Helper type to yield accounts from mnemonic
pub mod wallet;

/// Helper for payload operations
mod payload;

/// Helper for setting up nodes with pre-imported chain data
pub mod setup_import;

/// Helper for network operations
mod network;

/// Helper for rpc operations
mod rpc;

/// Utilities for creating and writing RLP test data
pub mod test_rlp_utils;

/// Builder for configuring test node setups
mod setup_builder;
pub use setup_builder::E2ETestSetupBuilder;

/// Creates the initial setup with `num_nodes` started and interconnected.
pub async fn setup<N>(
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    is_dev: bool,
) -> eyre::Result<(Vec<NodeHelperType<N>>, TaskManager, Wallet)>
where
    N: NodeBuilderHelper,
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes>,
{
    E2ETestSetupBuilder::new(num_nodes, chain_spec)
        .with_node_config_modifier(move |config| config.set_dev(is_dev))
        .build()
        .await
}

/// Creates the initial setup with `num_nodes` started and interconnected.
pub async fn setup_engine<N>(
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    is_dev: bool,
    tree_config: reth_node_api::TreeConfig,
) -> eyre::Result<(
    Vec<NodeHelperType<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
    TaskManager,
    Wallet,
)>
where
    N: NodeBuilderHelper,
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<N::Payload as PayloadTypes>::PayloadAttributes>,
{
    setup_engine_with_connection::<N>(num_nodes, chain_spec, is_dev, tree_config, true).await
}

/// Creates the initial setup with `num_nodes` started and optionally interconnected.
pub async fn setup_engine_with_connection<N>(
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    is_dev: bool,
    tree_config: reth_node_api::TreeConfig,
    connect_nodes: bool,
) -> eyre::Result<(
    Vec<NodeHelperType<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
    TaskManager,
    Wallet,
)>
where
    N: NodeBuilderHelper,
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<N::Payload as PayloadTypes>::PayloadAttributes>,
{
    E2ETestSetupBuilder::new(num_nodes, chain_spec)
        .with_tree_config_modifier(move |_| tree_config.clone())
        .with_node_config_modifier(move |config| config.set_dev(is_dev))
        .with_connect_nodes(connect_nodes)
        .build()
        .await
}

// Type aliases

/// Testing database
pub type TmpDB = Arc<TempDatabase<DatabaseEnv>>;
type TmpNodeAdapter<N, Provider = BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>> =
    FullNodeTypesAdapter<N, TmpDB, Provider>;

/// Type alias for a `NodeAdapter`
pub type Adapter<N, Provider = BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>> = NodeAdapter<
    TmpNodeAdapter<N, Provider>,
    <<N as Node<TmpNodeAdapter<N, Provider>>>::ComponentsBuilder as NodeComponentsBuilder<
        TmpNodeAdapter<N, Provider>,
    >>::Components,
>;

/// Type alias for a type of `NodeHelper`
pub type NodeHelperType<N, Provider = BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>> =
    NodeTestContext<Adapter<N, Provider>, <N as Node<TmpNodeAdapter<N, Provider>>>::AddOns>;

/// Helper trait to simplify bounds when calling setup functions.
pub trait NodeBuilderHelper
where
    Self: Default
        + NodeTypesForProvider
        + Node<
            TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            Primitives: NodePrimitives<
                BlockBody = alloy_consensus::BlockBody<
                    <Self::Primitives as NodePrimitives>::SignedTx,
                >,
            >,
            ComponentsBuilder: NodeComponentsBuilder<
                TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                Components: NodeComponents<
                    TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                    Network: PeersHandleProvider,
                >,
            >,
            AddOns: RethRpcAddOns<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            > + EngineValidatorAddOn<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            >,
            ChainSpec: From<ChainSpec> + Clone,
        >,
    LocalPayloadAttributesBuilder<Self::ChainSpec>:
        PayloadAttributesBuilder<<Self::Payload as PayloadTypes>::PayloadAttributes>,
{
}

impl<T> NodeBuilderHelper for T
where
    Self: Default
        + NodeTypesForProvider<
            Payload: PayloadTypes<
                PayloadBuilderAttributes: From<reth_payload_builder::EthPayloadBuilderAttributes>,
            >,
        > + Node<
            TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            Primitives: NodePrimitives<
                BlockHeader = alloy_consensus::Header,
                BlockBody = alloy_consensus::BlockBody<
                    <Self::Primitives as NodePrimitives>::SignedTx,
                >,
            >,
            ComponentsBuilder: NodeComponentsBuilder<
                TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                Components: NodeComponents<
                    TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                    Network: PeersHandleProvider,
                >,
            >,
            AddOns: RethRpcAddOns<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            > + EngineValidatorAddOn<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            >,
            ChainSpec: From<ChainSpec> + Clone,
        >,
    LocalPayloadAttributesBuilder<Self::ChainSpec>:
        PayloadAttributesBuilder<<Self::Payload as PayloadTypes>::PayloadAttributes>,
{
}
