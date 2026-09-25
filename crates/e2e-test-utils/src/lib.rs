//! Utilities for end-to-end tests.

use alloy_rpc_types_engine::PayloadAttributes;
use node::NodeTestContext;
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_builder::{
    components::NodeComponentsBuilder,
    rpc::{EngineValidatorAddOn, RethRpcAddOns},
    FullNodeTypesAdapter, Node, NodeAdapter, NodeComponents, NodeTypesWithDBAdapter, PayloadTypes,
};
use reth_provider::providers::{BlockchainProvider, NodeTypesForProvider};
use std::sync::Arc;

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

/// Helpers for verifying the persisted state and trie representation
pub mod trie;

mod chain_spec;
pub use chain_spec::{
    eth_payload_attributes, test_chain_spec, test_chain_spec_builder, test_genesis,
};

/// Builder for configuring test node setups
mod setup_builder;
pub use setup_builder::{E2ETestSetupBuilder, E2ETestSetupExt};

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

/// Helper trait to simplify the bounds of nodes launched by [`E2ETestSetupBuilder`].
pub trait NodeBuilderHelper
where
    Self: Default
        + NodeTypesForProvider<Payload: PayloadTypes<PayloadAttributes: From<PayloadAttributes>>>
        + Node<
            TmpNodeAdapter<Self>,
            ComponentsBuilder: NodeComponentsBuilder<
                TmpNodeAdapter<Self>,
                Components: NodeComponents<TmpNodeAdapter<Self>, Network: PeersHandleProvider>,
            >,
            AddOns: RethRpcAddOns<Adapter<Self>> + EngineValidatorAddOn<Adapter<Self>>,
        >,
{
}

impl<T> NodeBuilderHelper for T where
    Self: Default
        + NodeTypesForProvider<Payload: PayloadTypes<PayloadAttributes: From<PayloadAttributes>>>
        + Node<
            TmpNodeAdapter<Self>,
            ComponentsBuilder: NodeComponentsBuilder<
                TmpNodeAdapter<Self>,
                Components: NodeComponents<TmpNodeAdapter<Self>, Network: PeersHandleProvider>,
            >,
            AddOns: RethRpcAddOns<Adapter<Self>> + EngineValidatorAddOn<Adapter<Self>>,
        >
{
}
