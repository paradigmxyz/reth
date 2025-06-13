//! Wormhole Node Example
//!
//! This is a simplified example showing how to create a custom node with extended transaction
//! types. In this demo, we define a `WormholeTransaction` that extends Optimism transactions with a
//! placeholder for Wormhole-specific transactions.
//!
//! Note: This is a simplified implementation for demonstration purposes. A production
//! implementation would require:
//! - Proper Wormhole transaction validation and execution
//! - Custom EVM integration for Wormhole-specific opcodes
//! - Network protocol support for the new transaction types
//! - Updated mempool and consensus logic

pub mod evm;
pub mod pool;
pub mod primitives;

use reth_node_api::NodeTypes;
use reth_node_builder::{
    components::{BasicPayloadServiceBuilder, ComponentsBuilder},
    Node, NodeAdapter,
};
use reth_op::chainspec::{Hardforks, OpChainSpec};
use reth_optimism_forks::OpHardforks;
use reth_optimism_node::{
    node::OpPayloadBuilder as OpPayloadConfig, OpAddOns, OpConsensusBuilder, OpEngineApiBuilder,
    OpEngineValidatorBuilder, OpExecutorBuilder, OpNetworkBuilder, OpPoolBuilder,
};
use reth_optimism_rpc::eth::OpEthApiBuilder;

/// Example of extending Optimism with custom transaction types
/// This demonstrates the structure needed for custom transaction support
pub use primitives::{WormholeTransaction, WormholeTransactionSigned};
// pub use evm::{WormholeEvmConfig, WormholeBlockExecutor, WormholeBlockAssembler};

/// Wormhole node type
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WormholeNode {
    /// Enable transaction conditionals (demo: disabled)
    pub enable_tx_conditional: bool,
}

impl NodeTypes for WormholeNode {
    type Primitives = reth_op::OpPrimitives;
    type ChainSpec = OpChainSpec;
    type StateCommitment = reth_trie_db::MerklePatriciaTrie;
    type Storage = reth_optimism_node::OpStorage;
    type Payload = reth_optimism_node::OpEngineTypes;
}

impl WormholeNode {}

impl<N> Node<N> for WormholeNode
where
    N: reth_node_api::FullNodeTypes<
        Types: NodeTypes<
            Payload = reth_optimism_node::OpEngineTypes,
            ChainSpec: OpHardforks + Hardforks,
            Primitives = reth_op::OpPrimitives,
            Storage = reth_optimism_node::OpStorage,
        >,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        OpPoolBuilder,
        BasicPayloadServiceBuilder<OpPayloadConfig>,
        OpNetworkBuilder,
        OpExecutorBuilder,
        OpConsensusBuilder,
    >;
    type AddOns = OpAddOns<
        NodeAdapter<
            N,
            <Self::ComponentsBuilder as reth_node_builder::NodeComponentsBuilder<N>>::Components,
        >,
        OpEthApiBuilder,
        OpEngineValidatorBuilder,
        OpEngineApiBuilder<OpEngineValidatorBuilder>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(OpPoolBuilder::default())
            .executor(OpExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::new(OpPayloadConfig::new(false)))
            .network(OpNetworkBuilder::default())
            .consensus(OpConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        Self::AddOns::builder().with_enable_tx_conditional(self.enable_tx_conditional).build()
    }
}
