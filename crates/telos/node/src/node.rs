use crate::args::TelosArgs;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthEngineTypes, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{FullNodeTypes, NodeTypes};
use reth_node_builder::components::ComponentsBuilder;
use reth_node_builder::{Node, PayloadTypes};
use reth_node_ethereum::node::{
    EthereumAddOns, EthereumConsensusBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder,
    EthereumPayloadBuilder, EthereumPoolBuilder,
};

/// Type configuration for a regular Telos node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct TelosNode {
    /// Additional Telos args
    pub args: TelosArgs,
}

impl TelosNode {
    /// Creates a new instance of the Telos node type.
    pub const fn new(args: TelosArgs) -> Self {
        Self { args }
    }

    /// Returns a [`ComponentsBuilder`] configured for a regular Ethereum node.
    pub fn components<Node>() -> ComponentsBuilder<
        Node,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >
    where
        Node: FullNodeTypes,
        <Node as NodeTypes>::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::new(EthEvmConfig::default()))
            .network(EthereumNetworkBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }
}

impl NodeTypes for TelosNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
}

impl<N> Node<N> for TelosNode
where
    N: FullNodeTypes<Engine = EthEngineTypes>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    type AddOns = EthereumAddOns;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components()
    }
}
