#![allow(unused)]
use jsonrpsee_core::server::RpcModule;
use reth_arbitrum_rpc::ArbNitroRpc;

use super::args::RollupArgs;

#[derive(Debug, Clone, Default)]
pub struct ArbNode {
    pub args: RollupArgs,
}

impl ArbNode {
    pub fn new(rollup_args: RollupArgs) -> Self {
        Self { args: rollup_args }
    }

    pub fn arb_rpc_module() -> RpcModule<()> {
        ArbNitroRpc::default().into_rpc_module()
    }
}
use std::sync::Arc;

use reth_chainspec::ChainSpec;
use reth_node_api::{DebugNode, Node, NodeTypes, PayloadAttributesBuilder};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
        NetworkBuilder, PoolBuilder,
    },
    node::{FullNodeTypes, NodeTypes as _},
    rpc::{
        BasicEngineValidatorBuilder, EngineApiBuilder, EngineValidatorBuilder, EthApiBuilder,
        Identity, RethRpcAddOns, RethRpcMiddleware, RethRpcServerHandles, RpcAddOns, RpcContext,
        RpcHandle, RpcHooks,
    },
    BuilderContext, DebugNode as _, NodeAdapter,
};
use reth_payload_primitives::PayloadTypes;
use reth_provider::{providers::ProviderFactoryBuilder, EthStorage};
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
    type Storage = EthStorage;
    type Payload = crate::engine::ArbEngineTypes<reth_payload_builder::EthPayloadTypes>;
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
            ArbEvmConfig::new(Arc::new(ctx.chain_spec().clone()), ArbRethReceiptBuilder::default());
        Ok(evm_config)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbPoolBuilder;

impl<Types, N> PoolBuilder<N> for ArbPoolBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec>,
    N: FullNodeTypes<Types = Types>,
{
    type Pool = reth_transaction_pool::EthTransactionPool<
        N::Provider,
        reth_transaction_pool::blobstore::DiskFileBlobStore,
    >;

    async fn build_pool(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::Pool> {
        let blob_store =
            reth_node_builder::components::create_blob_store_with_cache(ctx, None)?;
        let validator =
            reth_transaction_pool::TransactionValidationTaskExecutor::eth_builder(
                ctx.provider().clone(),
            )
            .with_head_timestamp(ctx.head().timestamp)
            .with_max_tx_input_bytes(ctx.config().txpool.max_tx_input_bytes)
            .kzg_settings(ctx.kzg_settings()?)
            .with_local_transactions_config(
                ctx.pool_config().local_transactions_config.clone(),
            )
            .set_tx_fee_cap(ctx.config().rpc.rpc_tx_fee_cap)
            .with_max_tx_gas_limit(ctx.config().txpool.max_tx_gas_limit)
            .with_minimum_priority_fee(ctx.config().txpool.minimum_priority_fee)
            .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
            .build_with_tasks(ctx.task_executor().clone(), blob_store.clone());

        let pool = reth_node_builder::components::TxPoolBuilder::new(ctx)
            .with_validator(validator)
            .build_and_spawn_maintenance_task(blob_store, ctx.pool_config())?;
        Ok(pool)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbNetworkBuilder;

impl<N, Pool> NetworkBuilder<N, Pool> for ArbNetworkBuilder
where
    N: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
    Pool: reth_transaction_pool::TransactionPool + Unpin + 'static,
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
    N: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
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
    BasicPayloadServiceBuilder<reth_ethereum_node::EthereumPayloadBuilder>,
    ArbNetworkBuilder,
    ArbExecutorBuilder,
    ArbConsensusBuilder,
>;

#[derive(Debug)]
pub struct ArbAddOns<
    N: FullNodeComponents,
    EthB: EthApiBuilder<N> = reth_ethereum_node::EthereumEthApiBuilder,
    PVB = (),
    EB = ArbEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<PVB>,
    RpcM = Identity,
> {
    pub rpc_add_ons: RpcAddOns<N, EthB, PVB, EB, EVB, RpcM>,
}

impl<N> Default for ArbAddOns<N>
where
    N: FullNodeComponents,
    reth_ethereum_node::EthereumEthApiBuilder: EthApiBuilder<N>,
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

impl<N, EthB, PVB, EB, EVB, Attrs, RpcM> reth_node_api::NodeAddOns<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
    Attrs: serde::de::DeserializeOwned,
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
                modules.merge(arb_rpc.into_rpc())?;
                Ok(())
            })
            .await
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> RethRpcAddOns<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcM: RethRpcMiddleware,
{
    type EthApi = EthB::EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi> {
        self.rpc_add_ons.hooks_mut()
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
        reth_ethereum_node::EthereumEthApiBuilder,
        (),
        ArbEngineApiBuilder<BasicEngineValidatorBuilder<()>>,
        BasicEngineValidatorBuilder<()>,
        Identity,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(ArbPoolBuilder::default())
            .executor(ArbExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::default())
            .network(ArbNetworkBuilder::default())
            .consensus(ArbConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        let engine_api = ArbEngineApiBuilder {
            engine_validator_builder: BasicEngineValidatorBuilder::default(),
        };
        ArbAddOns::default().with_engine_api(engine_api)
    }
}

impl<N> DebugNode<N> for ArbNode
where
    N: FullNodeComponents<Types = Self>,
{
    type RpcBlock = alloy_rpc_types_eth::Block;

    fn rpc_to_primitive_block(
        rpc_block: Self::RpcBlock,
    ) -> reth_node_api::BlockTy<Self> {
        rpc_block.into_consensus()
    }

    fn local_payload_attributes_builder(
        chain_spec: &Self::ChainSpec,
    ) -> impl PayloadAttributesBuilder<
        <Self::Payload as PayloadTypes>::PayloadAttributes,
    > {
        reth_engine_local::LocalPayloadAttributesBuilder::new(Arc::new(
            chain_spec.clone(),
        ))
    }
}
