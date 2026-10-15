//! reth-bb: a modified reth node for benchmarking big block execution.
#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

mod evm;
mod evm_config;

use alloy_primitives::Bytes;
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
use tracing::info;

#[derive(Debug, Clone, Default)]
pub struct BbPayloadTypes;

impl PayloadTypes for BbPayloadTypes {
    type ExecutionData = BigBlockData<ExecutionData>;
    type BuiltPayload = <EthPayloadTypes as PayloadTypes>::BuiltPayload;
    type PayloadAttributes = <EthPayloadTypes as PayloadTypes>::PayloadAttributes;

    fn block_to_payload(
        block: SealedBlock<
                <<Self::BuiltPayload as reth_node_api::BuiltPayload>::Primitives as reth_node_api::NodePrimitives>::Block,
            >,
        bal: Option<Bytes>,
    ) -> Self::ExecutionData {
        BigBlockData {
            block_number: block.number,
            // Historical headers have no BAL hash, so the inner payload conversion drops the
            // supplied BAL. Keep it separately without changing the original block hash.
            env_switches: vec![EthPayloadTypes::block_to_payload(block, bal.clone())],
            prior_block_hashes: vec![],
            merged_block_access_list: bal,
        }
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
        let mut blocks = payload
            .env_switches
            .into_iter()
            .map(|data| {
                PayloadValidator::<EthPayloadTypes>::convert_payload_to_block(&self.inner, data)
            })
            .collect::<Result<Vec<SealedBlock<Block>>, NewPayloadError>>()?;

        let (mut block, hash) = blocks.pop().unwrap().split();

        // Override the block number
        block.header.number = payload.block_number;

        // Set block's parent hash to the parent of the first block in this batch so that engine
        // tree state is consistent.
        if let Some(first) = blocks.first() {
            block.header.parent_hash = first.parent_hash;
        }

        // Update block's gas usage to make sure metrics are correct
        block.header.gas_used += blocks.iter().map(|b| b.gas_used).sum::<u64>();
        block.header.gas_limit += blocks.iter().map(|b| b.gas_limit).sum::<u64>();

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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_engine_primitives::ExecutionPayload as _;
    use reth_engine_tree::tree::payload_validator::BlockOrPayload;
    use reth_primitives_traits::Block as _;

    #[test]
    fn historical_block_replay_preserves_bal() {
        let mut block = Block::default();
        block.header.number = 42;
        block.header.base_fee_per_gas = Some(1);
        let block = block.seal_slow();
        let bal = Bytes::from_static(&[0xc0]);
        let payload = BbPayloadTypes::block_to_payload(block.clone(), Some(bal.clone()));

        assert_eq!(payload.block_access_list(), Some(&bal));
        assert_eq!(payload.block_number(), block.number);
        assert_eq!(payload.block_hash(), block.hash());
        assert_eq!(payload.env_switches.len(), 1);
        assert_eq!(payload.env_switches[0].payload.block_access_list(), None);
        assert_eq!(payload.prior_block_hashes, vec![]);

        let input = BlockOrPayload::<BbPayloadTypes>::Payload(payload.clone());
        assert!(input.has_block_access_list());
        assert!(input.try_decoded_access_list().unwrap().unwrap().as_bal().is_empty());

        let validator =
            BbEngineValidator { inner: EthereumEngineValidator::new(ChainSpec::default().into()) };
        let converted = validator.convert_payload_to_block(payload).unwrap();
        assert_eq!(converted, block);
        assert_eq!(converted.block_access_list_hash, None);
    }

    #[test]
    fn block_replay_without_bal_preserves_payload() {
        let block = Block::default().seal_slow();
        let payload = BbPayloadTypes::block_to_payload(block.clone(), None);

        assert_eq!(payload.block_access_list(), None);
        assert_eq!(payload.block_hash(), block.hash());
        assert_eq!(payload.env_switches.len(), 1);
        let input = BlockOrPayload::<BbPayloadTypes>::Payload(payload);
        assert!(!input.has_block_access_list());
        assert!(input.try_decoded_access_list().unwrap().is_none());
    }
}
