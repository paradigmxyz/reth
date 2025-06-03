//! This example shows how implement a custom node.
//!
//! A node consists of:
//! - primitives: block,header,transactions
//! - components: network,pool,evm
//! - engine: advances the node

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use crate::{
    evm::CustomExecutorBuilder, pool::CustomPooledTransaction, primitives::CustomTransaction,
};
use chainspec::CustomChainSpec;
use primitives::CustomNodePrimitives;
use reth_ethereum::node::api::{FullNodeTypes, NodeTypes};
use reth_node_builder::{
    components::{BasicPayloadServiceBuilder, ComponentsBuilder},
    Node,
};
use reth_op::node::{
    node::{OpConsensusBuilder, OpNetworkBuilder, OpPayloadBuilder, OpPoolBuilder},
    txpool, OpNode, OpPayloadTypes,
};

pub mod chainspec;
pub mod engine;
pub mod engine_api;
pub mod evm;
pub mod pool;
pub mod primitives;

#[derive(Debug, Clone)]
pub struct CustomNode {}

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = <OpNode as NodeTypes>::StateCommitment;
    type Storage = <OpNode as NodeTypes>::Storage;
    type Payload = OpPayloadTypes<CustomNodePrimitives>;
}

impl<N> Node<N> for CustomNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        OpPoolBuilder<txpool::OpPooledTransaction<CustomTransaction, CustomPooledTransaction>>,
        BasicPayloadServiceBuilder<OpPayloadBuilder>,
        OpNetworkBuilder,
        CustomExecutorBuilder,
        OpConsensusBuilder,
    >;

    type AddOns = ();

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(OpPoolBuilder::default())
            .executor(CustomExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::new(OpPayloadBuilder::new(false)))
            .network(OpNetworkBuilder::new(false, false))
            .consensus(OpConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {}
}
