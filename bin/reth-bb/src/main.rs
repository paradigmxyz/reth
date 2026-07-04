//! reth-bb: a modified reth node for benchmarking big block execution.
#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

mod evm;
mod evm_config;

use alloy_consensus::constants::MAXIMUM_EXTRA_DATA_SIZE;
use alloy_eips::Decodable2718;
use alloy_primitives::Bytes;
use alloy_rpc_types::engine::{
    ExecutionData, ExecutionPayload as AlloyExecutionPayload, PayloadError,
};
use clap::Parser;
use evm_config::{BbEvmConfig, BigBlockData};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_consensus::noop::NoopConsensus;
use reth_ethereum_cli::{chainspec::EthereumChainSpecParser, interface::Cli};
use reth_ethereum_primitives::{Block, EthPrimitives, TransactionSigned};
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
        let tx_capacity = payload.env_switches.iter().map(ExecutionData::transaction_count).sum();
        let mut env_switches = payload.env_switches.into_iter();
        let first = env_switches.next().expect("big block payload has at least one env switch");
        let last = env_switches.next_back();

        let mut previous_gas_used = 0;
        let mut previous_gas_limit = 0;
        let mut previous_transactions = Vec::with_capacity(tx_capacity);
        let mut parent_hash = None;

        let terminal = if let Some(last) = last {
            let first =
                PayloadValidator::<EthPayloadTypes>::convert_payload_to_block(&self.inner, first)?;
            parent_hash = Some(first.parent_hash);
            previous_gas_used += first.gas_used;
            previous_gas_limit += first.gas_limit;
            previous_transactions.extend(first.into_body().transactions);

            for data in env_switches {
                let (gas_used, gas_limit, transactions) = decode_intermediate_payload(data)?;
                previous_gas_used += gas_used;
                previous_gas_limit += gas_limit;
                previous_transactions.extend(transactions);
            }

            last
        } else {
            first
        };
        let (mut block, hash) =
            PayloadValidator::<EthPayloadTypes>::convert_payload_to_block(&self.inner, terminal)?
                .split();

        // Override the block number
        block.header.number = payload.block_number;

        // Set block's parent hash to the parent of the first block in this batch so that engine
        // tree state is consistent.
        if let Some(parent_hash) = parent_hash {
            block.header.parent_hash = parent_hash;
        }

        // Update block's gas usage to make sure metrics are correct
        block.header.gas_used += previous_gas_used;
        block.header.gas_limit += previous_gas_limit;

        // Prepend transactions from previous blocks to make sure that persistence indices are
        // correct.
        previous_transactions.append(&mut block.body.transactions);
        block.body.transactions = previous_transactions;

        // Use `new_unchecked` to preserve the hash
        Ok(SealedBlock::new_unchecked(block, hash))
    }
}

fn decode_intermediate_payload(
    data: ExecutionData,
) -> Result<(u64, u64, Vec<TransactionSigned>), NewPayloadError> {
    let ExecutionData { payload, .. } = data;
    let payload = match payload {
        AlloyExecutionPayload::V1(payload) => payload,
        AlloyExecutionPayload::V2(payload) => payload.payload_inner,
        AlloyExecutionPayload::V3(payload) => payload.payload_inner.payload_inner,
        AlloyExecutionPayload::V4(payload) => payload.payload_inner.payload_inner.payload_inner,
    };

    if payload.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        return Err(PayloadError::ExtraData(payload.extra_data).into());
    }

    let base_fee_per_gas = payload.base_fee_per_gas;
    let _: u64 =
        base_fee_per_gas.try_into().map_err(|_| PayloadError::BaseFee(base_fee_per_gas))?;

    let transactions = payload
        .transactions
        .into_iter()
        .map(|tx| {
            TransactionSigned::decode_2718_exact(tx.as_ref())
                .map_err(alloy_rlp::Error::from)
                .map_err(PayloadError::from)
        })
        .collect::<Result<_, _>>()?;

    Ok((payload.gas_used, payload.gas_limit, transactions))
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
