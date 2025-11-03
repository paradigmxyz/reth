//! Example for a node maintaining custom database index allowing to optimize RPC queries.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use crate::rpc::EthApiOverrideServer;
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        api::{AddOnsContext, FullNodeTypes, NodeAddOns, NodeTypes, NodeTypesWithDBAdapter},
        builder::{
            components::{BasicPayloadServiceBuilder, ComponentsBuilder},
            rpc::{RethRpcAddOns, RpcHandle},
            Node, NodeAdapter,
        },
        EthEngineTypes, EthereumAddOns, EthereumConsensusBuilder, EthereumEngineValidatorBuilder,
        EthereumEthApiBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder, EthereumNode,
        EthereumPayloadBuilder, EthereumPoolBuilder,
    },
    provider::{
        db::{database_metrics::DatabaseMetrics, Database},
        providers::BlockchainProvider,
    },
    rpc::eth::EthApiFor,
    EthPrimitives,
};

mod storage;
use storage::CustomStorage;

use crate::rpc::EthApiOverrides;

mod provider;

mod rpc;

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

#[derive(Default)]
pub struct CustomAddOns<N: FullNodeTypes<Types = CustomNode>> {
    inner: EthereumAddOns<NodeAdapter<N>, EthereumEthApiBuilder, EthereumEngineValidatorBuilder>,
}

impl<DB, N> NodeAddOns<NodeAdapter<N>> for CustomAddOns<N>
where
    DB: Database + Clone + Unpin + DatabaseMetrics + 'static,
    N: FullNodeTypes<
        Types = CustomNode,
        DB = DB,
        Provider = BlockchainProvider<NodeTypesWithDBAdapter<CustomNode, DB>>,
    >,
{
    type Handle = RpcHandle<NodeAdapter<N>, EthApiFor<NodeAdapter<N>>>;

    async fn launch_add_ons(
        mut self,
        ctx: AddOnsContext<'_, NodeAdapter<N>>,
    ) -> eyre::Result<Self::Handle> {
        self.inner.hooks_mut().set_extend_rpc_modules(|ctx| {
            let overrides = EthApiOverrides { inner: ctx.registry.eth_api().clone() };
            ctx.modules.replace_http(overrides.into_rpc())?;
            Ok(())
        });
        self.inner.launch_add_ons(ctx).await
    }
}
