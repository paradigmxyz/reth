//! reth-bb: a modified reth node for benchmarking big block execution.
#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

mod evm;
mod evm_config;

use alloy_primitives::B256;

use alloy_rpc_types::engine::{ExecutionData, ForkchoiceState, ForkchoiceUpdated};
use async_trait::async_trait;
use clap::Parser;
use evm_config::{BbEvmConfig, BigBlockData};
use jsonrpsee::core::RpcResult;
use reth_chainspec::{ChainSpec, EthereumHardforks, Hardforks};
use reth_consensus::noop::NoopConsensus;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_ethereum_cli::{chainspec::EthereumChainSpecParser, interface::Cli};
use reth_ethereum_primitives::EthPrimitives;
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{AddOnsContext, FullNodeComponents, NodeTypes, PayloadTypes};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
    },
    node::FullNodeTypes,
    rpc::{
        BasicEngineApiBuilder, BasicEngineValidatorBuilder, EngineApiBuilder, EngineValidatorAddOn,
        EngineValidatorBuilder, PayloadValidatorBuilder, RethRpcAddOns, RpcAddOns, RpcHandle,
        RpcHooks,
    },
    BuilderContext, Node,
};
use reth_node_ethereum::{
    EthEngineTypes, EthereumEngineValidatorBuilder, EthereumEthApiBuilder, EthereumNetworkBuilder,
    EthereumNode, EthereumPayloadBuilder, EthereumPoolBuilder,
};
use reth_payload_primitives::ExecutionPayload;
use reth_primitives_traits::SealedBlock;
use reth_provider::EthStorage;
use reth_rpc_api::{RethNewPayloadInput, RethPayloadStatus};
use reth_rpc_engine_api::EngineApiError;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::{info, trace};

/// Shared map for big block data, keyed by payload hash.
pub type BigBlockMap = Arc<Mutex<HashMap<B256, BigBlockData<ExecutionData>>>>;

// ---------------------------------------------------------------------------
// Custom RPC trait for big-block payloads
// ---------------------------------------------------------------------------

/// Big-block extension of the `reth_` engine API.
#[jsonrpsee::proc_macros::rpc(server, namespace = "reth")]
pub trait BbRethEngineApi {
    /// `reth_newPayload` with optional big-block data.
    #[method(name = "newPayload")]
    async fn reth_new_payload(
        &self,
        payload: RethNewPayloadInput<ExecutionData>,
        wait_for_persistence: Option<bool>,
        wait_for_caches: Option<bool>,
        big_block_data: Option<BigBlockData<ExecutionData>>,
    ) -> RpcResult<RethPayloadStatus>;

    /// `reth_forkchoiceUpdated` – pass-through.
    #[method(name = "forkchoiceUpdated")]
    async fn reth_forkchoice_updated(
        &self,
        forkchoice_state: ForkchoiceState,
    ) -> RpcResult<ForkchoiceUpdated>;
}

/// Server-side implementation of `BbRethEngineApi`.
#[derive(Debug)]
struct BbRethEngineApiHandler {
    pending: BigBlockMap,
    engine: ConsensusEngineHandle<EthEngineTypes>,
}

#[async_trait]
impl BbRethEngineApiServer for BbRethEngineApiHandler {
    async fn reth_new_payload(
        &self,
        input: RethNewPayloadInput<ExecutionData>,
        wait_for_persistence: Option<bool>,
        wait_for_caches: Option<bool>,
        big_block_data: Option<BigBlockData<ExecutionData>>,
    ) -> RpcResult<RethPayloadStatus> {
        let wait_for_persistence = wait_for_persistence.unwrap_or(true);
        let wait_for_caches = wait_for_caches.unwrap_or(true);
        trace!(
            target: "rpc::engine",
            wait_for_persistence,
            wait_for_caches,
            has_big_block_data = big_block_data.is_some(),
            "Serving bb reth_newPayload"
        );

        let payload = match input {
            RethNewPayloadInput::ExecutionData(data) => data,
            RethNewPayloadInput::BlockRlp(rlp) => {
                let block = alloy_rlp::Decodable::decode(&mut rlp.as_ref())
                    .map_err(|err| EngineApiError::Internal(Box::new(err)))?;
                <EthEngineTypes as PayloadTypes>::block_to_payload(SealedBlock::new_unhashed(block))
            }
        };

        if let Some(data) = big_block_data {
            let hash = ExecutionPayload::block_hash(&payload);
            self.pending.lock().unwrap().insert(hash, data);
        }

        let (status, timings) = self
            .engine
            .reth_new_payload(payload, wait_for_persistence, wait_for_caches)
            .await
            .map_err(EngineApiError::from)?;

        Ok(RethPayloadStatus {
            status,
            latency_us: timings.latency.as_micros() as u64,
            persistence_wait_us: timings.persistence_wait.as_micros() as u64,
            execution_cache_wait_us: timings.execution_cache_wait.map(|d| d.as_micros() as u64),
            sparse_trie_wait_us: timings.sparse_trie_wait.map(|d| d.as_micros() as u64),
        })
    }

    async fn reth_forkchoice_updated(
        &self,
        forkchoice_state: ForkchoiceState,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving reth_forkchoiceUpdated");
        self.engine
            .fork_choice_updated(forkchoice_state, None)
            .await
            .map_err(|e| EngineApiError::from(e).into())
    }
}

// ---------------------------------------------------------------------------
// Node add-ons wrapper
// ---------------------------------------------------------------------------

/// Add-ons for the big-block node.
#[derive(Debug)]
pub struct BbAddOns {
    pending: BigBlockMap,
}

impl BbAddOns {
    const fn new(pending: BigBlockMap) -> Self {
        Self { pending }
    }

    fn make_rpc_add_ons<N: FullNodeComponents>(
        &self,
    ) -> RpcAddOns<
        N,
        EthereumEthApiBuilder,
        EthereumEngineValidatorBuilder,
        BasicEngineApiBuilder<EthereumEngineValidatorBuilder>,
        BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>,
    >
    where
        EthereumEthApiBuilder: reth_node_builder::rpc::EthApiBuilder<N>,
    {
        RpcAddOns::new(
            EthereumEthApiBuilder::default(),
            EthereumEngineValidatorBuilder::default(),
            BasicEngineApiBuilder::default(),
            BasicEngineValidatorBuilder::default(),
            Default::default(),
            Default::default(),
        )
    }
}

impl<N> reth_node_api::NodeAddOns<N> for BbAddOns
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks + Hardforks + Clone + 'static,
            Payload = EthEngineTypes,
            Primitives = EthPrimitives,
        >,
    >,
    EthereumEthApiBuilder: reth_node_builder::rpc::EthApiBuilder<N>,
    EthereumEngineValidatorBuilder: PayloadValidatorBuilder<N>,
    BasicEngineApiBuilder<EthereumEngineValidatorBuilder>: EngineApiBuilder<N>,
    BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>: EngineValidatorBuilder<N>,
{
    type Handle =
        RpcHandle<N, <EthereumEthApiBuilder as reth_node_builder::rpc::EthApiBuilder<N>>::EthApi>;

    async fn launch_add_ons(self, ctx: AddOnsContext<'_, N>) -> eyre::Result<Self::Handle> {
        let engine_handle = ctx.beacon_engine_handle.clone();
        let pending = self.pending.clone();
        let rpc_add_ons = self.make_rpc_add_ons::<N>();

        rpc_add_ons
            .launch_add_ons_with(ctx, move |container| {
                let handler = BbRethEngineApiHandler { pending, engine: engine_handle };
                let bb_module = BbRethEngineApiServer::into_rpc(handler);
                container.auth_module.replace_auth_methods(bb_module.remove_context())?;
                Ok(())
            })
            .await
    }
}

impl<N> RethRpcAddOns<N> for BbAddOns
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks + Hardforks + Clone + 'static,
            Payload = EthEngineTypes,
            Primitives = EthPrimitives,
        >,
    >,
    EthereumEthApiBuilder: reth_node_builder::rpc::EthApiBuilder<N>,
    EthereumEngineValidatorBuilder: PayloadValidatorBuilder<N>,
    BasicEngineApiBuilder<EthereumEngineValidatorBuilder>: EngineApiBuilder<N>,
    BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>: EngineValidatorBuilder<N>,
{
    type EthApi = <EthereumEthApiBuilder as reth_node_builder::rpc::EthApiBuilder<N>>::EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi> {
        unimplemented!("BbAddOns does not support dynamic hook mutation")
    }
}

impl<N> EngineValidatorAddOn<N> for BbAddOns
where
    N: FullNodeComponents,
    BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>: EngineValidatorBuilder<N>,
{
    type ValidatorBuilder = BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>;

    fn engine_validator_builder(&self) -> Self::ValidatorBuilder {
        BasicEngineValidatorBuilder::default()
    }
}

// ---------------------------------------------------------------------------
// Custom executor builder
// ---------------------------------------------------------------------------

/// Executor builder that creates a [`BbEvmConfig`].
#[derive(Debug)]
pub struct BbExecutorBuilder {
    pending: BigBlockMap,
}

impl<Node> ExecutorBuilder<Node> for BbExecutorBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec: reth_ethereum_forks::Hardforks
                           + alloy_evm::eth::spec::EthExecutorSpec
                           + EthereumHardforks,
            Primitives = EthPrimitives,
        >,
    >,
{
    type EVM = BbEvmConfig<<Node::Types as NodeTypes>::ChainSpec>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        Ok(BbEvmConfig::new(EthEvmConfig::new(ctx.chain_spec()), self.pending))
    }
}

// ---------------------------------------------------------------------------
// Node type
// ---------------------------------------------------------------------------

/// Node type for big block execution.
#[derive(Debug, Clone)]
pub struct BbNode {
    pending: BigBlockMap,
}

impl BbNode {
    const fn new(pending: BigBlockMap) -> Self {
        Self { pending }
    }
}

impl NodeTypes for BbNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage;
    type Payload = EthEngineTypes;
}

impl<N> Node<N> for BbNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        EthereumNetworkBuilder,
        BbExecutorBuilder,
        BbConsensusBuilder,
    >;

    type AddOns = BbAddOns;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        EthereumNode::components()
            .executor(BbExecutorBuilder { pending: self.pending.clone() })
            .consensus(BbConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        BbAddOns::new(self.pending.clone())
    }
}

// ---------------------------------------------------------------------------
// Consensus builder
// ---------------------------------------------------------------------------

/// Consensus builder for big block execution.
#[derive(Debug, Default, Clone, Copy)]
pub struct BbConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for BbConsensusBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    type Consensus = NoopConsensus;

    async fn build_consensus(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(NoopConsensus::default())
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    let pending: BigBlockMap = Arc::new(Mutex::new(HashMap::new()));

    if let Err(err) = Cli::<EthereumChainSpecParser>::parse().run(async move |builder, _| {
        info!(target: "reth::cli", "Launching big block node");
        let handle = builder.launch_node(BbNode::new(pending.clone())).await?;

        handle.wait_for_node_exit().await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
