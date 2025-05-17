use crate::{
    chainspec::CustomChainSpec, consensus::CustomConsensusBuilder, engine::CustomPayloadTypes,
    executor::CustomExecutorBuilder, payload::CustomTxPriority, pool::CustomPoolBuilder,
    primitives::CustomNodePrimitives,
};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_ethereum::node::api::{FullNodeTypes, NodeTypes};
use reth_node_builder::{
    components::{BasicPayloadServiceBuilder, ComponentsBuilder},
    Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_op::node::{
    args::RollupArgs,
    node::{OpNetworkBuilder, OpPayloadBuilder, OpStorage},
    OpNode,
};
use reth_optimism_node::node::OpAddOns;
use reth_optimism_rpc::eth::OpEthApiBuilder;

#[derive(Debug, Clone)]
pub struct CustomNode {}

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = <OpNode as NodeTypes>::StateCommitment;
    type Storage = <OpNode as NodeTypes>::Storage;
    type Payload = CustomPayloadTypes;
}

impl<N> Node<N> for CustomNode
where
    N: FullNodeTypes<
        Types: NodeTypes<
            Payload = CustomPayloadTypes,
            ChainSpec = CustomChainSpec,
            Primitives = CustomNodePrimitives,
            Storage = OpStorage,
        >,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        CustomPoolBuilder,
        CustomExecutorBuilder,
        BasicPayloadServiceBuilder<OpPayloadBuilder>,
        OpNetworkBuilder,
        CustomConsensusBuilder,
    >;

    type AddOns = OpAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
        OpEthApiBuilder,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        let RollupArgs { disable_txpool_gossip, compute_pending_block, discovery_v4, .. } =
            RollupArgs::default();

        let chain_id = self.chain_spec().chain().id();

        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(CustomPoolBuilder::default())
            .executor(CustomExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::new(
                OpPayloadBuilder::new(compute_pending_block)
                    .with_transactions(CustomTxPriority::new(chain_id)),
            ))
            .network(OpNetworkBuilder {
                disable_txpool_gossip,
                disable_discovery_v4: !discovery_v4,
            })
            .consensus(CustomConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {}
}
