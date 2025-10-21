//! Example for a node maintaining custom database index allowing to optimize RPC queries.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{
            components::{BasicPayloadServiceBuilder, ComponentsBuilder},
            Node, NodeAdapter,
        },
        EthEngineTypes, EthereumAddOns, EthereumConsensusBuilder, EthereumEngineValidatorBuilder,
        EthereumEthApiBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder, EthereumNode,
        EthereumPayloadBuilder, EthereumPoolBuilder,
    },
    EthPrimitives,
};

use crate::storage::CustomStorage;

mod storage;

#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomNode(EthereumNode);

impl NodeTypes for CustomNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = CustomStorage;
    type Payload = EthEngineTypes;
}

impl<N> Node<N> for CustomNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;
    type AddOns =
        EthereumAddOns<NodeAdapter<N>, EthereumEthApiBuilder, EthereumEngineValidatorBuilder>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        EthereumNode::components()
    }

    fn add_ons(&self) -> Self::AddOns {
        EthereumAddOns::default()
    }
}
