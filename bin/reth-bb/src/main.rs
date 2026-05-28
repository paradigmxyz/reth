//! reth-bb: a modified reth node for benchmarking big block execution.
#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

mod evm;
mod evm_config;

use alloy_rpc_types::engine::ExecutionData;
use clap::Parser;
use evm_config::{BbEvmConfig, BigBlockData};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_consensus::noop::NoopConsensus;
use reth_ethereum_cli::{chainspec::EthereumChainSpecParser, interface::Cli};
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{
    AddOnsContext, FullNodeComponents, NewPayloadError, NodeTypes, PayloadTypes, PayloadValidator,
};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
    },
    node::FullNodeTypes,
    rpc::{NoopEngineApiBuilder, PayloadValidatorBuilder, RpcAddOns},
    BuilderContext, Node, NodeAdapter,
};
use reth_node_core::args::DefaultEngineValues;
use reth_node_ethereum::{
    EthPayloadTypes, EthereumEngineValidator, EthereumEthApiBuilder, EthereumNetworkBuilder,
    EthereumNode, EthereumPayloadBuilder, EthereumPoolBuilder,
};
use reth_primitives_traits::SealedBlock;
use reth_provider::EthStorage;
use revm_primitives::Bytes;
use tracing::info;

#[derive(Debug, Clone, Default)]
pub struct BbPayloadTypes;

impl PayloadTypes for BbPayloadTypes {
    type ExecutionData = BigBlockData<ExecutionData>;
    type BuiltPayload = <EthPayloadTypes as PayloadTypes>::BuiltPayload;
    type PayloadAttributes = <EthPayloadTypes as PayloadTypes>::PayloadAttributes;

    fn block_to_payload(
        _block: SealedBlock<
                <<Self::BuiltPayload as reth_node_api::BuiltPayload>::Primitives as reth_node_api::NodePrimitives>::Block,
            >,
        _bal: Option<Bytes>,
    ) -> Self::ExecutionData {
        unreachable!()
    }
}

#[derive(Debug, Default, Clone)]
pub struct BbEngineValidatorBuilder;

impl<Node> PayloadValidatorBuilder<Node> for BbEngineValidatorBuilder
where
    Node: FullNodeComponents<Types = BbNode>,
{
    type Validator = BbEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(BbEngineValidator { inner: EthereumEngineValidator::new(ctx.config.chain.clone()) })
    }
}

#[derive(Debug, Clone)]
pub struct BbEngineValidator {
    inner: EthereumEngineValidator,
}

impl PayloadValidator<BbPayloadTypes> for BbEngineValidator {
    type Block = Block;

    fn convert_payload_to_block(
        &self,
        payload: BigBlockData<ExecutionData>,
    ) -> Result<SealedBlock<Block>, NewPayloadError> {
        let segment_count = payload.env_switches.len();
        let last_segment_index = segment_count.saturating_sub(1);
        let mut preceding_gas_used = 0u64;
        let mut first_parent_hash = None;
        let mut blocks = Vec::with_capacity(segment_count);

        for (idx, data) in payload.env_switches.into_iter().enumerate() {
            let block =
                PayloadValidator::<EthPayloadTypes>::convert_payload_to_block(&self.inner, data)?;
            if idx == 0 && segment_count > 1 {
                first_parent_hash = Some(block.parent_hash);
            }
            if idx < last_segment_index {
                preceding_gas_used += block.gas_used;
            }
            blocks.push(block);
        }

        let (mut block, hash) = blocks.pop().unwrap().split();

        // Override the block number
        block.header.number = payload.block_number;

        // Set block's parent hash to the parent of the first block in this batch so that engine
        // tree state is consistent.
        if let Some(parent_hash) = first_parent_hash {
            block.header.parent_hash = parent_hash;
        }

        // Update block's gas usage to make sure metrics are correct
        block.header.gas_used += preceding_gas_used;

        // Prepend transactions from previous blocks to make sure that persistence indices are
        // correct.
        block.body.transactions = blocks
            .into_iter()
            .flat_map(|b| b.into_body().transactions)
            .chain(core::mem::take(&mut block.body.transactions))
            .collect();

        // Use `new_unchecked` to preserve the hash
        Ok(SealedBlock::new_unchecked(block, hash))
    }
}

// ---------------------------------------------------------------------------
// Custom executor builder
// ---------------------------------------------------------------------------

/// Executor builder that creates a [`BbEvmConfig`].
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BbExecutorBuilder;

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
        Ok(BbEvmConfig::new(EthEvmConfig::new(ctx.chain_spec())))
    }
}

// ---------------------------------------------------------------------------
// Node type
// ---------------------------------------------------------------------------

/// Node type for big block execution.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BbNode;

impl NodeTypes for BbNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage;
    type Payload = BbPayloadTypes;
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

    type AddOns = RpcAddOns<
        NodeAdapter<N>,
        EthereumEthApiBuilder,
        BbEngineValidatorBuilder,
        NoopEngineApiBuilder,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        EthereumNode::components()
            .executor(BbExecutorBuilder::default())
            .consensus(BbConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        Default::default()
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

    let _ = DefaultEngineValues::default().with_bal_parallel_execution_disabled(false).try_init();

    if let Err(err) = Cli::<EthereumChainSpecParser>::parse().run(async move |builder, _| {
        info!(target: "reth::cli", "Launching big block node");
        let handle = builder.launch_node(BbNode::default()).await?;

        handle.wait_for_node_exit().await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
