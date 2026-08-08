//! Example: Running a reth node with a custom block execution hook.
//!
//! This demonstrates how to install a [`CustomBlockExecutionHook`] on the serial (non-BAL)
//! execution path. The example always returns `Ok(None)` so the validator falls back to the
//! default execution path — it only shows builder wiring.
//!
//! The key integration point is wrapping [`BasicEngineValidatorBuilder`] to call
//! [`BasicEngineValidator::with_custom_block_execution_hook`] on the resulting validator.
//!
//! For a full-block cache hit (e.g. reusing flashblock execution keyed by `input.env.hash`),
//! return [`CustomBlockExecutionOutput`] with a pre-resolved `receipt_root_rx`. See the type
//! docs for state-root implications on that path.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p example-custom-block-executor -- node --dev --http
//! ```

#![warn(unused_crate_dependencies)]

use std::sync::Arc;

use alloy_genesis::Genesis;
use reth_engine_tree::tree::{
    payload_validator::CustomBlockExecutionHook, BasicEngineValidator, TreeConfig,
};
use reth_ethereum::{
    chainspec::ChainSpec,
    evm::factory::RethEvmFactory,
    node::{
        builder::{
            rpc::{
                BasicEngineApiBuilder, BasicEngineValidatorBuilder, EngineValidatorBuilder,
                Identity, RpcAddOns,
            },
            FullNodeComponents, NodeBuilder, NodeHandle,
        },
        core::{args::RpcServerArgs, node_config::NodeConfig},
        EthEvmConfig, EthereumAddOns, EthereumEngineValidatorBuilder, EthereumEthApiBuilder,
        EthereumNode,
    },
    tasks::Runtime,
    EthPrimitives,
};
use reth_storage_overlay::OverlayManager;

type ExampleEvmConfig = EthEvmConfig<ChainSpec, RethEvmFactory>;

// ---------------------------------------------------------------------------
// Custom Engine Validator Builder
// ---------------------------------------------------------------------------

/// An [`EngineValidatorBuilder`] that wraps [`BasicEngineValidatorBuilder`] and
/// installs a [`CustomBlockExecutionHook`] on the resulting [`BasicEngineValidator`].
#[derive(Clone)]
struct PassthroughBlockExecutionHookValidatorBuilder {
    inner: BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>,
    custom_block_execution_hook: CustomBlockExecutionHook<EthPrimitives, ExampleEvmConfig>,
}

impl std::fmt::Debug for PassthroughBlockExecutionHookValidatorBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassthroughBlockExecutionHookValidatorBuilder")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<N> EngineValidatorBuilder<N> for PassthroughBlockExecutionHookValidatorBuilder
where
    N: FullNodeComponents<Types = EthereumNode, Evm = ExampleEvmConfig>,
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
        overlay_manager: OverlayManager<EthPrimitives>,
    ) -> eyre::Result<Self::EngineValidator> {
        let validator = self.inner.build_tree_validator(ctx, tree_config, overlay_manager).await?;
        Ok(validator.with_custom_block_execution_hook(self.custom_block_execution_hook))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let runtime = Runtime::test();

    // Always fall back to the default path. Replace with a cache lookup keyed by
    // `input.env.hash` to reuse prior full-block execution.
    let passthrough_hook: CustomBlockExecutionHook<EthPrimitives, ExampleEvmConfig> =
        Arc::new(|input| {
            let _block_hash = input.env.hash;
            Ok(None)
        });

    let node_config = NodeConfig::test()
        .dev()
        .with_rpc(RpcServerArgs::default().with_http())
        .with_chain(custom_chain());

    let add_ons: EthereumAddOns<_, _, _, _, PassthroughBlockExecutionHookValidatorBuilder> =
        EthereumAddOns::new(RpcAddOns::new(
            EthereumEthApiBuilder::<alloy_network::Ethereum>::default(),
            EthereumEngineValidatorBuilder::default(),
            BasicEngineApiBuilder::<EthereumEngineValidatorBuilder>::default(),
            PassthroughBlockExecutionHookValidatorBuilder {
                inner: BasicEngineValidatorBuilder::default(),
                custom_block_execution_hook: passthrough_hook,
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

    println!("Node running with custom block execution hook — press Ctrl+C to exit");

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
