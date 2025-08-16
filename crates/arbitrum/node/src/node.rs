use reth_arbitrum_rpc::ArbNitroRpc;
use reth_arbitrum_rpc::ArbNitroApiServer;

use crate::follower::DynFollowerExecutor;
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
use reth_provider::{StateProviderFactory, HeaderProvider};
use crate::follower::FollowerExecutor;
use reth_evm::ConfigureEvm;
use reth_payload_primitives::BuildNextEnv;
use reth_provider::ChainSpecProvider;

use alloy_eips::eip2718::Decodable2718;
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
            ArbEvmConfig::new(ctx.config().chain.clone(), ArbRethReceiptBuilder::default());
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
    crate::conditional_payload::ConditionalPayloadServiceBuilder<
        BasicPayloadServiceBuilder<crate::payload::ArbPayloadBuilderBuilder>
    >,
    ArbNetworkBuilder,
    ArbExecutorBuilder,
    ArbConsensusBuilder,
>;

#[derive(Clone)]
pub struct ArbFollowerExec<N: FullNodeComponents> {
    pub provider: N::Provider,
    pub beacon: reth_node_api::ConsensusEngineHandle<<N::Types as NodeTypes>::Payload>,
    pub evm_config: reth_arbitrum_evm::ArbEvmConfig<ChainSpec, reth_arbitrum_primitives::ArbPrimitives>,
}

impl<N> FollowerExecutor for ArbFollowerExec<N>
where
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>> + Send + Sync + 'static,
{
    fn execute_message_to_block(
        &self,
        parent_hash: alloy_primitives::B256,
        attrs: alloy_rpc_types_engine::PayloadAttributes,
        l2msg_bytes: &[u8],
    ) -> core::pin::Pin<Box<dyn core::future::Future<Output = eyre::Result<(alloy_primitives::B256, alloy_primitives::B256)>> + Send>> {
        use reth_payload_primitives::EngineApiMessageVersion;
        use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatusEnum};
        use reth_revm::{database::StateProviderDatabase, db::State};
        use reth_evm::execute::BlockBuilder;

        let provider = self.provider.clone();
        let evm_config = self.evm_config.clone();
        let beacon = self.beacon.clone();
        let l2_owned: Vec<u8> = l2msg_bytes.to_vec();

        Box::pin(async move {
            let _ = beacon
                .fork_choice_updated(
                    ForkchoiceState {
                        head_block_hash: parent_hash,
                        safe_block_hash: parent_hash,
                        finalized_block_hash: parent_hash,
                    },
                    None,
                    EngineApiMessageVersion::default(),
                )
                .await?;

            let parent_header = provider
                .header(&parent_hash)?
                .ok_or_else(|| eyre::eyre!("missing parent header"))?;
            let sealed_parent = reth_primitives_traits::SealedHeader::new(parent_header, parent_hash);

            let (exec_data, block_hash, send_root) = {
                let state_provider = provider.state_by_block_hash(parent_hash)?;
                let mut db = State::builder()
                    .with_database(StateProviderDatabase::new(&state_provider))
                    .with_bundle_update()
                    .build();

                let next_env = <reth_arbitrum_evm::ArbEvmConfig<ChainSpec, reth_arbitrum_primitives::ArbPrimitives> as reth_evm::ConfigureEvm>::NextBlockEnvCtx::build_next_env(
                    &reth_payload_builder::EthPayloadBuilderAttributes::new(parent_hash, attrs.clone().into()),
                    &sealed_parent,
                    evm_config.chain_spec().as_ref(),
                ).map_err(|e| eyre::eyre!("build_next_env error: {e}"))?;

                let mut builder = evm_config
                    .builder_for_next_block(&mut db, &sealed_parent, next_env)
                    .map_err(|e| eyre::eyre!("builder_for_next_block error: {e}"))?;

                builder.apply_pre_execution_changes().map_err(|e| eyre::eyre!("apply_pre_execution_changes: {e}"))?;

                let mut txs: Vec<reth_arbitrum_primitives::ArbTransactionSigned> = Vec::new();
                if !l2_owned.is_empty() {
                    let kind = l2_owned[0];
                    let mut cur = &l2_owned[1..];
                    if kind == 0x00 {
                        let (env, used) = arb_alloy_consensus::tx::ArbTxEnvelope::decode_typed(cur)
                            .map_err(|e| eyre::eyre!("decode_typed: {e}"))?;
                        let _ = used;
                        match env {
                            arb_alloy_consensus::tx::ArbTxEnvelope::Legacy(bytes) => {
                                let mut s = bytes.as_slice();
                                let tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                                    .map_err(|_| eyre::eyre!("failed to decode ArbTransactionSigned"))?;
                                txs.push(tx);
                            }
                            _ => {
                                let mut buf = env.encode_typed();
                                let mut s = buf.as_slice();
                                let tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                                    .map_err(|_| eyre::eyre!("failed to decode ArbTransactionSigned from typed env"))?;
                                txs.push(tx);
                            }
                        }
                    } else {
                        while cur.len() >= 8 {
                            let mut len_bytes = [0u8; 8];
                            len_bytes.copy_from_slice(&cur[..8]);
                            cur = &cur[8..];
                            let seg_len = u64::from_be_bytes(len_bytes) as usize;
                            if cur.len() < seg_len { break; }
                            let seg = &cur[..seg_len];
                            cur = &cur[seg_len..];
                            if seg.is_empty() { continue; }
                            let _seg_kind = seg[0];
                            let seg_payload = &seg[1..];
                            match arb_alloy_consensus::tx::ArbTxEnvelope::decode_typed(seg_payload) {
                                Ok((env, _used)) => {
                                    let mut buf = env.encode_typed();
                                    let mut s = buf.as_slice();
                                    if let Ok(tx) = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s) {
                                        txs.push(tx);
                                    }
                                }
                                Err(_) => {
                                    let mut s = seg_payload;
                                    if let Ok(tx) = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s) {
                                        txs.push(tx);
                                    }
                                }
                            }
                        }
                    }
                }

                let outcome = builder.finish(&state_provider).map_err(|e| eyre::eyre!("finish error: {e}"))?;
                let sealed_block = outcome.block.sealed_block().clone();

                let block_hash = sealed_block.hash();
                let header = sealed_block.header();
                let send_root = reth_arbitrum_evm::header::extract_send_root_from_header_extra(header.extra_data.as_ref());

                let exec_data =
                    <<N as reth_node_api::FullNodeTypes>::Types as reth_node_api::NodeTypes>::Payload::block_to_payload(sealed_block);
                (exec_data, block_hash, send_root)
            };

            let res = beacon.new_payload(exec_data).await?;
            match res.status {
                PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted | PayloadStatusEnum::Syncing => {}
                other => return Err(eyre::eyre!("new_payload returned status {other:?}")),
            }

            let fcu = ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };
            let fcu_res = beacon.fork_choice_updated(fcu, None, EngineApiMessageVersion::default()).await?;
            if !matches!(fcu_res.payload_status.status, PayloadStatusEnum::Valid | PayloadStatusEnum::Syncing) {
                return Err(eyre::eyre!("fork_choice_updated not valid/syncing: {:?}", fcu_res.payload_status));
            }
            Ok((block_hash, send_root))
        })
    }
}
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
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>>,
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
        let rpc_handle = rpc_add_ons
            .launch_add_ons_with(ctx.clone(), move |container| {
                let reth_node_builder::rpc::RpcModuleContainer { modules, .. } = container;
                let arb_rpc = ArbNitroRpc::default();
                modules.merge_configured(arb_rpc.into_rpc())?;
                Ok(())
            })
            .await?;

        let follower: ArbFollowerExec<N> = ArbFollowerExec {
            provider: ctx.node.provider().clone(),
            beacon: rpc_handle.beacon_engine_handle.clone(),
            evm_config: reth_arbitrum_evm::ArbEvmConfig::new(ctx.config.chain.clone(), reth_arbitrum_evm::ArbRethReceiptBuilder::default()),
        };
        crate::follower::set_follower_executor(Arc::new(follower));
        Ok(rpc_handle)
    }
}

impl<N, EthB, PVB, EB, EVB, RpcM> RethRpcAddOns<N>
    for ArbAddOns<N, EthB, PVB, EB, EVB, RpcM>
where
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = reth_arbitrum_primitives::ArbPrimitives>>,
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
            .payload(crate::conditional_payload::ConditionalPayloadServiceBuilder::new(
                BasicPayloadServiceBuilder::new(crate::payload::ArbPayloadBuilderBuilder::default()),
                self.args.sequencer,
            ))
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
