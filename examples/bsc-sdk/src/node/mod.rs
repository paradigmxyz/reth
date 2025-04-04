use crate::{chainspec::BscChainSpec, node::rpc::BscEthApiBuilder};
use evm::BscExecutorBuilder;
use network::BscNetworkBuilder;
use payload::builder::BscPayloadBuilder;
use pool::BscPoolBuilder;
use reth::{
    api::{FullNodeComponents, FullNodeTypes, NodeTypes},
    beacon_consensus::EthBeaconConsensus,
    builder::{
        components::{BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder},
        rpc::{EngineValidatorBuilder, RpcAddOns},
        AddOnsContext, BuilderContext, DebugNode, Node, NodeAdapter, NodeComponentsBuilder,
    },
    consensus::{ConsensusError, FullConsensus},
};
use reth_node_ethereum::{EthEngineTypes, EthereumEngineValidator};

use reth_primitives::{Block, BlockBody, EthPrimitives};

use reth_provider::{providers::ProviderFactoryBuilder, EthStorage};

use reth_trie_db::MerklePatriciaTrie;

use std::sync::Arc;

pub mod evm;
pub mod network;
pub mod payload;
pub mod pool;
pub mod rpc;

/// Bsc addons configuring RPC types
pub type BscNodeAddOns<N> = RpcAddOns<N, BscEthApiBuilder, BscEngineValidatorBuilder>;

/// A basic ethereum consensus builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BscConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for BscConsensusBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>>,
{
    type Consensus = Arc<dyn FullConsensus<EthPrimitives, Error = ConsensusError>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec())))
    }
}

/// Builder for [`EthereumEngineValidator`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for BscEngineValidatorBuilder
where
    Types:
        NodeTypes<ChainSpec = BscChainSpec, Payload = EthEngineTypes, Primitives = EthPrimitives>,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = EthereumEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(EthereumEngineValidator::new(Arc::new(ctx.config.chain.clone().as_ref().clone().into())))
    }
}

/// Type configuration for a regular BSC node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscNode;

impl BscNode {
    pub fn components<Node>(
        &self,
    ) -> ComponentsBuilder<
        Node,
        BscPoolBuilder,
        BasicPayloadServiceBuilder<BscPayloadBuilder>,
        BscNetworkBuilder,
        BscExecutorBuilder,
        BscConsensusBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypes<
                Payload = EthEngineTypes,
                ChainSpec = BscChainSpec,
                Primitives = EthPrimitives,
            >,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(BscPoolBuilder::default())
            .payload(BasicPayloadServiceBuilder::default())
            .network(BscNetworkBuilder::default())
            .executor(BscExecutorBuilder::default())
            .consensus(BscConsensusBuilder::default())
    }

    pub fn provider_factory_builder() -> ProviderFactoryBuilder<Self> {
        ProviderFactoryBuilder::default()
    }
}

impl<N> Node<N> for BscNode
where
    N: FullNodeTypes<
        Types: NodeTypes<
            Payload = EthEngineTypes,
            ChainSpec = BscChainSpec,
            Primitives = EthPrimitives,
            Storage = EthStorage,
        >,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        BscPoolBuilder,
        BasicPayloadServiceBuilder<BscPayloadBuilder>,
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
    type Payload = EthEngineTypes;
}
