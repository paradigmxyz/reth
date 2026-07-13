//! Example: Running a reth node with a custom state root strategy.
//!
//! This demonstrates how to install a custom state-root strategy that activates at a fork
//! timestamp. Blocks with a timestamp below the activation delegate to
//! [`DefaultStateRootStrategy`]; blocks at or above it return `B256::ZERO` as the state root.
//!
//! The key integration points are:
//! - wrapping [`BasicEngineValidatorBuilder`] to call
//!   [`BasicEngineValidator::with_state_root_strategy`] on the resulting validator, and
//! - holding a [`DefaultStateRootStrategy`] inside the custom strategy and forwarding the context
//!   to it for blocks where the default behavior is wanted.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p example-custom-state-root -- node --dev --http
//! ```

#![warn(unused_crate_dependencies)]

use std::sync::Arc;

use alloy_genesis::Genesis;
use alloy_primitives::B256;
use reth_chain_state::StateTrieOverlayManager;
use reth_engine_tree::tree::{
    state_root_strategy::{
        DefaultStateRootStrategy, LazyHashedPostState, PayloadStateRootHandle,
        PayloadStateRootJobContext, PreparedStateRootJob, StateRootJob, StateRootJobContext,
        StateRootJobOutcome, StateRootStrategy,
    },
    BasicEngineValidator, TreeConfig,
};
use reth_ethereum::{
    chainspec::ChainSpec,
    evm::EthEvmEnv,
    node::{
        builder::{
            rpc::{
                BasicEngineApiBuilder, BasicEngineValidatorBuilder, ChangesetCache,
                EngineValidatorBuilder, Identity, RpcAddOns,
            },
            FullNodeComponents, NodeBuilder, NodeHandle,
        },
        core::{args::RpcServerArgs, node_config::NodeConfig},
        EthereumAddOns, EthereumEngineValidatorBuilder, EthereumEthApiBuilder, EthereumNode,
    },
    tasks::Runtime,
    EthPrimitives,
};
use reth_evm::{BlockExecutorFactory, ConfigureEvm, TxEnvFor};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock};
use reth_provider::{BlockExecutionOutput, ProviderResult};
use reth_trie::updates::TrieUpdates;

/// Strategy that returns `B256::ZERO` as the state root from an activation timestamp on, and
/// delegates to the default machinery before that.
#[derive(Debug)]
struct ZeroStateRootStrategy {
    /// Timestamp of the first block that uses the custom state root.
    activation_timestamp: u64,
    /// Default strategy used for blocks below the activation timestamp.
    default: DefaultStateRootStrategy,
}

impl<N, P, Evm> StateRootStrategy<N, P, Evm> for ZeroStateRootStrategy
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
    Evm::BlockExecutorFactory: BlockExecutorFactory<EvmEnv = EthEvmEnv>,
    DefaultStateRootStrategy: StateRootStrategy<N, P, Evm>,
{
    fn prepare(
        &self,
        ctx: StateRootJobContext<'_, N, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<N>> {
        let timestamp: u64 = ctx.env().evm_env.block.timestamp.saturating_to();
        if timestamp < self.activation_timestamp {
            return self.default.prepare(ctx)
        }
        Ok(PreparedStateRootJob::new(Box::new(ZeroStateRootJob), None))
    }

    fn prepare_payload_builder(
        &self,
        ctx: PayloadStateRootJobContext<'_, N, P>,
    ) -> ProviderResult<Option<PayloadStateRootHandle>> {
        if ctx.timestamp() < self.activation_timestamp {
            return self.default.prepare_payload_builder(ctx)
        }
        // Without a background task the payload builder computes the state root itself. A real
        // strategy would return a custom handle here so built headers match validation.
        Ok(None)
    }
}

#[derive(Debug)]
struct ZeroStateRootJob;

impl<N> StateRootJob<N> for ZeroStateRootJob
where
    N: NodePrimitives,
{
    fn name(&self) -> &'static str {
        "zero"
    }

    fn finish(
        &mut self,
        _block: &RecoveredBlock<N::Block>,
        _output: Arc<BlockExecutionOutput<N::Receipt>>,
        _hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        Ok(StateRootJobOutcome::new(B256::ZERO, Arc::new(TrieUpdates::default())))
    }
}

/// An [`EngineValidatorBuilder`] that wraps [`BasicEngineValidatorBuilder`] and
/// installs a custom state-root strategy on the resulting [`BasicEngineValidator`].
#[derive(Clone)]
struct ZeroStateRootValidatorBuilder {
    inner: BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>,
    state_root_strategy: Arc<ZeroStateRootStrategy>,
}

impl std::fmt::Debug for ZeroStateRootValidatorBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZeroStateRootValidatorBuilder")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<N> EngineValidatorBuilder<N> for ZeroStateRootValidatorBuilder
where
    N: FullNodeComponents<
        Types = EthereumNode,
        Evm: reth_ethereum::node::builder::ConfigureEngineEvm<
            alloy_rpc_types_engine::ExecutionData,
        >,
    >,
    <N::Evm as ConfigureEvm>::BlockExecutorFactory: BlockExecutorFactory<EvmEnv = EthEvmEnv>,
    TxEnvFor<N::Evm>: Clone + Send + 'static,
{
    type EngineValidator = BasicEngineValidator<
        N::Provider,
        N::Evm,
        <EthereumEngineValidatorBuilder as reth_ethereum::node::builder::rpc::PayloadValidatorBuilder<N>>::Validator,
    >;

    async fn build_tree_validator(
        self,
        ctx: &reth_ethereum::node::builder::AddOnsContext<'_, N>,
        tree_config: TreeConfig,
        changeset_cache: ChangesetCache,
        state_trie_overlays: StateTrieOverlayManager<EthPrimitives>,
    ) -> eyre::Result<Self::EngineValidator> {
        let validator = self
            .inner
            .build_tree_validator(ctx, tree_config, changeset_cache, state_trie_overlays)
            .await?;
        let state_root_strategy: Arc<dyn StateRootStrategy<EthPrimitives, N::Provider, N::Evm>> =
            self.state_root_strategy;
        Ok(validator.with_state_root_strategy(state_root_strategy))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let runtime = Runtime::test();

    let node_config = NodeConfig::test()
        .dev()
        .with_rpc(RpcServerArgs::default().with_http())
        .with_chain(custom_chain());

    // Build add-ons with our custom engine validator builder.
    let add_ons: EthereumAddOns<_, _, _, _, ZeroStateRootValidatorBuilder> =
        EthereumAddOns::new(RpcAddOns::new(
            EthereumEthApiBuilder::<alloy_network::Ethereum>::default(),
            EthereumEngineValidatorBuilder::default(),
            BasicEngineApiBuilder::<EthereumEngineValidatorBuilder>::default(),
            ZeroStateRootValidatorBuilder {
                inner: BasicEngineValidatorBuilder::default(),
                state_root_strategy: Arc::new(ZeroStateRootStrategy {
                    // Zero roots from genesis on. Set this to a fork timestamp to keep the
                    // default state-root machinery for earlier blocks.
                    activation_timestamp: 0,
                    default: DefaultStateRootStrategy::default(),
                }),
            },
            Default::default(),
            Identity::new(),
        ));

    let NodeHandle { node: _node, node_exit_future } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(add_ons)
        .launch_with_debug_capabilities()
        .await?;

    println!("Node running with custom zero state root — press Ctrl+C to exit");

    node_exit_future.await
}

fn custom_chain() -> Arc<ChainSpec> {
    let custom_genesis = r#"
{
    "nonce": "0x42",
    "timestamp": "0x0",
    "extraData": "0x5343",
    "gasLimit": "0x1c9c380",
    "difficulty": "0x0",
    "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "coinbase": "0x0000000000000000000000000000000000000000",
    "alloc": {
        "0x6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b": {
            "balance": "0x4a47e3c12448f4ad000000"
        }
    },
    "number": "0x0",
    "gasUsed": "0x0",
    "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "config": {
        "ethash": {},
        "chainId": 2600,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "berlinBlock": 0,
        "londonBlock": 0,
        "terminalTotalDifficulty": 0,
        "terminalTotalDifficultyPassed": true,
        "shanghaiTime": 0
    }
}
"#;
    let genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    Arc::new(genesis.into())
}
