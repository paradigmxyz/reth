#![allow(unused)]
use reth_arbitrum_rpc::ArbNitroRpc;
use reth_arbitrum_rpc::ArbNitroApiServer;

use super::args::RollupArgs;

#[derive(Debug, Clone, Default)]
pub struct ArbNode {
    pub args: RollupArgs,
}

impl ArbNode {
    pub fn new(rollup_args: RollupArgs) -> Self {
        Self { args: rollup_args }
    }
}
use std::sync::Arc;

use reth_chainspec::ChainSpec;
use reth_node_api::{FullNodeComponents, NodeTypes, PayloadAttributesBuilder, PayloadTypes};
use reth_node_builder::{DebugNode, Node};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
        NetworkBuilder, PoolBuilder, NoopPayloadBuilder,
    },
    node::{FullNodeTypes, NodeTypes as _},
    rpc::{
        BasicEngineValidatorBuilder, EngineApiBuilder, EngineValidatorBuilder, EthApiBuilder,
        Identity, RethRpcAddOns, RethRpcMiddleware, RethRpcServerHandles, RpcAddOns, RpcContext,
        RpcHandle, RpcHooks,
    },
    BuilderContext, DebugNode as _, NodeAdapter,
};
use reth_provider::providers::ProviderFactoryBuilder;
use reth_arbitrum_storage::ArbStorage;
use reth_rpc_api::servers::DebugApiServer;
use reth_rpc_server_types::RethRpcModule;
use reth_trie_db::MerklePatriciaTrie;

use reth_arbitrum_evm::{ArbEvmConfig, ArbRethReceiptBuilder};
use reth_arbitrum_payload::ArbExecutionData;
use crate::engine::ArbEngineTypes;
use crate::rpc::ArbEngineApiBuilder;
impl NodeTypes for ArbNode {
    type Primitives = reth_arbitrum_primitives::ArbPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = ArbStorage;
    type Payload = crate::engine::ArbEngineTypes<reth_arbitrum_payload::ArbPayloadTypes>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbExecutorBuilder;

impl<Types, N> ExecutorBuilder<N> for ArbExecutorBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>,
    N: FullNodeTypes<Types = Types>,
{
    type EVM = ArbEvmConfig<ChainSpec, Types::Primitives>;

    async fn build_evm(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::EVM> {
        let evm_config =
            ArbEvmConfig::new(ctx.chain_spec().clone(), ArbRethReceiptBuilder::default());
        Ok(evm_config)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbPoolBuilder;

impl<Types, N> PoolBuilder<N> for ArbPoolBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>,
    N: FullNodeTypes<Types = Types>,
{
    type Pool = reth_transaction_pool::noop::NoopTransactionPool<reth_arbitrum_txpool::ArbPooledTransaction>;

    async fn build_pool(self, _ctx: &BuilderContext<N>) -> eyre::Result<Self::Pool> {
        Ok(reth_transaction_pool::noop::NoopTransactionPool::new())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbNetworkBuilder;

impl<N, Pool> NetworkBuilder<N, Pool> for ArbNetworkBuilder
where
    N: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
    Pool: reth_transaction_pool::TransactionPool<
            Transaction: reth_transaction_pool::PoolTransaction<
                Consensus = reth_node_api::TxTy<N::Types>
            >
        > + Unpin + 'static,
{
    type Network = reth_network::NetworkHandle<
        reth_network::primitives::BasicNetworkPrimitives<
            reth_node_api::PrimitivesTy<N::Types>,
            reth_transaction_pool::PoolPooledTx<Pool>,
        >,
    >;

    async fn build_network(
        self,
        ctx: &BuilderContext<N>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
        let network = ctx.network_builder().await?;
        let handle = ctx.start_network(network, pool);
        Ok(handle)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbConsensusBuilder;

impl<N> ConsensusBuilder<N> for ArbConsensusBuilder
where
    N: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>>,
{
    type Consensus =
        std::sync::Arc<reth_ethereum_consensus::EthBeaconConsensus<ChainSpec>>;

    async fn build_consensus(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::Consensus> {
        Ok(std::sync::Arc::new(
            reth_ethereum_consensus::EthBeaconConsensus::new(ctx.chain_spec()),
        ))
    }
}

pub type ArbNodeComponents<N> = ComponentsBuilder<
    N,
    ArbPoolBuilder,
    reth_node_builder::components::NoopPayloadServiceBuilder,
    ArbNetworkBuilder,
    ArbExecutorBuilder,
    ArbConsensusBuilder,
>;

#[derive(Debug)]
pub struct ArbAddOns<
    N: FullNodeComponents,
    EthB: EthApiBuilder<N> = reth_arbitrum_rpc::ArbEthApiBuilder,
    PVB = (),
    EB = ArbEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<crate::validator::ArbPayloadValidatorBuilder>,
    RpcM = Identity,
> {
    pub rpc_add_ons: RpcAddOns<N, EthB, PVB, EB, EVB, RpcM>,
}

impl<N> Default for ArbAddOns<N>
where
    N: FullNodeComponents,
    reth_arbitrum_rpc::ArbEthApiBuilder: EthApiBuilder<N>,
{
    fn default() -> Self {
        Self { rpc_add_ons: RpcAddOns::default() }
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
{
    pub fn with_engine_api<T>(
        self,
        engine_api_builder: T,
    ) -> ArbAddOns<N, EthB, PVB, T, EVB, RpcM> {
        ArbAddOns { rpc_add_ons: self.rpc_add_ons.with_engine_api(engine_api_builder) }
    }

    pub fn with_payload_validator<T>(
        self,
        payload_validator_builder: T,
    ) -> ArbAddOns<N, EthB, T, EB, EVB, RpcM> {
        ArbAddOns {
            rpc_add_ons: self
                .rpc_add_ons
                .with_payload_validator(payload_validator_builder),
        }
    }

    pub fn with_rpc_middleware<T>(
        self,
        rpc_middleware: T,
    ) -> ArbAddOns<N, EthB, PVB, EB, EVB, T> {
        ArbAddOns { rpc_add_ons: self.rpc_add_ons.with_rpc_middleware(rpc_middleware) }
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> reth_node_api::NodeAddOns<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents<Types: NodeTypes<ChainSpec: reth_chainspec::EthereumHardforks>>,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
{
    type Handle = RpcHandle<N, EthB::EthApi>;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        let rpc_add_ons = self.rpc_add_ons;
        rpc_add_ons
            .launch_add_ons_with(ctx, move |container| {
                let reth_node_builder::rpc::RpcModuleContainer { modules, .. } = container;
                let arb_rpc = ArbNitroRpc::default();
                modules.merge_configured(arb_rpc.into_rpc())?;
                Ok(())
            })
            .await
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> RethRpcAddOns<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents<Types: NodeTypes<ChainSpec: reth_chainspec::EthereumHardforks>>,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
{
    type EthApi = EthB::EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi> {
        &mut self.rpc_add_ons.hooks
    }
}
impl<N, EthB, PVB, EB, EVB, RpcM> reth_node_builder::rpc::EngineValidatorAddOn<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N> + Default,
    RpcM: RethRpcMiddleware,
{
    type ValidatorBuilder = EVB;

    fn engine_validator_builder(&self) -> Self::ValidatorBuilder {
        EVB::default()
    }
}


impl<N> Node<N> for ArbNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ArbNodeComponents<N>;

    type AddOns = ArbAddOns<
        NodeAdapter<
            N,
            <Self::ComponentsBuilder as reth_node_builder::components::NodeComponentsBuilder<N>>::Components,
        >,
        reth_arbitrum_rpc::ArbEthApiBuilder,
        (),
        ArbEngineApiBuilder<crate::validator::ArbPayloadValidatorBuilder>,
        BasicEngineValidatorBuilder<crate::validator::ArbPayloadValidatorBuilder>,
        Identity,
    >;

    fn components_builder(&self) -> <Self as Node<N>>::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(ArbPoolBuilder::default())
            .executor(ArbExecutorBuilder::default())
            .payload(reth_node_builder::components::NoopPayloadServiceBuilder::default())
            .network(ArbNetworkBuilder::default())
            .consensus(ArbConsensusBuilder::default())
    }

    fn add_ons(&self) -> <Self as Node<N>>::AddOns {
        let engine_api = ArbEngineApiBuilder::new(
            crate::validator::ArbPayloadValidatorBuilder::default(),
        );
        ArbAddOns::default().with_engine_api(engine_api)
    }
}

impl<N> DebugNode<N> for ArbNode
where
    N: FullNodeComponents<Types = Self>,
{
    type RpcBlock = alloy_rpc_types_eth::Block<reth_arbitrum_primitives::ArbTransactionSigned>;

    fn rpc_to_primitive_block(
        rpc_block: <Self as DebugNode<N>>::RpcBlock,
    ) -> reth_node_api::BlockTy<Self> {
        rpc_block.into_consensus()
    }

    fn local_payload_attributes_builder(
        chain_spec: &<Self as reth_node_api::NodeTypes>::ChainSpec,
    ) -> impl PayloadAttributesBuilder<
        <Self::Payload as PayloadTypes>::PayloadAttributes,
    > {
        reth_engine_local::LocalPayloadAttributesBuilder::new(Arc::new(
            chain_spec.clone(),
        ))
    }
}
