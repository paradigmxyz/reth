use crate::{chainspec::BscChainSpec, node::rpc::BscEthApiBuilder};

use crate::node::rpc::engine_api::{
    builder::BscEngineApiBuilder, payload::BscPayloadTypes, validator::BscEngineValidatorBuilder,
};
use consensus::BscConsensusBuilder;
use evm::BscExecutorBuilder;
use network::BscNetworkBuilder;
use reth::{
    api::{FullNodeComponents, FullNodeTypes, NodeTypes},
    builder::{
        components::{BasicPayloadServiceBuilder, ComponentsBuilder},
        rpc::RpcAddOns,
        DebugNode, Node, NodeAdapter, NodeComponentsBuilder,
    },
};
use reth_node_ethereum::node::{EthereumPayloadBuilder, EthereumPoolBuilder};
use reth_primitives::{Block, BlockBody, EthPrimitives};
use reth_provider::EthStorage;
use reth_trie_db::MerklePatriciaTrie;

pub mod cli;
pub mod consensus;
pub mod evm;
pub mod network;
pub mod rpc;

/// Bsc addons configuring RPC types
pub type BscNodeAddOns<N> =
    RpcAddOns<N, BscEthApiBuilder, BscEngineValidatorBuilder, BscEngineApiBuilder>;

/// Type configuration for a regular BSC node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscNode;

impl BscNode {
    pub fn components<Node>(
        &self,
    ) -> ComponentsBuilder<
        Node,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        BscNetworkBuilder,
        BscExecutorBuilder,
        BscConsensusBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypes<
                Payload = BscPayloadTypes,
                ChainSpec = BscChainSpec,
                Primitives = EthPrimitives,
            >,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .executor(BscExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::default())
            .network(BscNetworkBuilder::default())
            .consensus(BscConsensusBuilder::default())
    }
}

impl<N> Node<N> for BscNode
where
    N: FullNodeTypes<
        Types: NodeTypes<
            Payload = BscPayloadTypes,
            ChainSpec = BscChainSpec,
            Primitives = EthPrimitives,
            Storage = EthStorage,
        >,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        BscNetworkBuilder,
        BscExecutorBuilder,
        BscConsensusBuilder,
    >;

    type AddOns = BscNodeAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components(self)
    }

    fn add_ons(&self) -> Self::AddOns {
        BscNodeAddOns::default()
    }
}

impl<N> DebugNode<N> for BscNode
where
    N: FullNodeComponents<Types = Self>,
{
    type RpcBlock = alloy_rpc_types::Block;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Block {
        let alloy_rpc_types::Block { header, transactions, withdrawals, .. } = rpc_block;
        Block {
            header: header.inner,
            body: BlockBody {
                transactions: transactions
                    .into_transactions()
                    .map(|tx| tx.inner.into_inner().into())
                    .collect(),
                ommers: Default::default(),
                withdrawals,
            },
        }
    }
}

impl NodeTypes for BscNode {
    type Primitives = EthPrimitives;
    type ChainSpec = BscChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = EthStorage;
    type Payload = BscPayloadTypes;
}
