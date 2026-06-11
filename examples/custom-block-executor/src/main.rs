//! Example: Running a reth node with a custom block execution hook.
//!
//! This demonstrates how to use the [`CustomBlockExecutor`] hook on the serial (non-BAL)
//! execution path. The example always returns `Ok(None)` so the validator falls back to the
//! default execution path.
//!
//! The key integration point is wrapping [`BasicEngineValidatorBuilder`] to call
//! [`BasicEngineValidator::with_custom_block_executor`] on the resulting validator.
//!
//! For a full cache hit (e.g. reusing flashblock execution), return
//! [`CustomBlockExecutionOutput`] with a pre-resolved `receipt_root_rx`. For incremental
//! execution inside the hook, call [`CustomBlockExecutorInput::spawn_receipt_root_task`] and
//! stream receipts to the sender.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p example-custom-block-executor -- node --dev --http
//! ```

#![warn(unused_crate_dependencies)]

use std::sync::Arc;

use alloy_genesis::Genesis;
use reth_engine_tree::tree::{payload_validator::CustomBlockExecutor, BasicEngineValidator};
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        EthEvmConfig,
        builder::{
            rpc::{
                BasicEngineApiBuilder, BasicEngineValidatorBuilder, ChangesetCache,
                EngineValidatorBuilder, Identity, RpcAddOns,
            },
            FullNodeComponents, NodeBuilder, NodeHandle, TreeConfig,
        },
        core::{args::RpcServerArgs, node_config::NodeConfig},
        EthereumAddOns, EthereumEngineValidatorBuilder, EthereumEthApiBuilder, EthereumNode,
    },
    tasks::Runtime,
    EthPrimitives,
};

// ---------------------------------------------------------------------------
// Custom Engine Validator Builder
// ---------------------------------------------------------------------------

/// An [`EngineValidatorBuilder`] that wraps [`BasicEngineValidatorBuilder`] and
/// installs a [`CustomBlockExecutor`] on the resulting [`BasicEngineValidator`].
#[derive(Clone)]
struct PassthroughBlockExecutorValidatorBuilder {
    inner: BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>,
    custom_block_executor: CustomBlockExecutor<EthPrimitives, EthEvmConfig<ChainSpec>>,
}

impl std::fmt::Debug for PassthroughBlockExecutorValidatorBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassthroughBlockExecutorValidatorBuilder")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<N> EngineValidatorBuilder<N> for PassthroughBlockExecutorValidatorBuilder
where
    N: FullNodeComponents<
        Types = EthereumNode,
        Evm = EthEvmConfig<ChainSpec>,
    >,
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
    ) -> eyre::Result<Self::EngineValidator> {
        let validator = self.inner.build_tree_validator(ctx, tree_config, changeset_cache).await?;
        Ok(validator.with_custom_block_executor(self.custom_block_executor))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let runtime = Runtime::test();

    // A custom block executor that always falls back to the default path.
    // Replace with cache lookup keyed by `input.env.hash` to reuse prior execution.
    let passthrough_executor: CustomBlockExecutor<EthPrimitives, EthEvmConfig<ChainSpec>> =
        Arc::new(|input| {
            let _block_hash = input.env.hash;
            Ok(None)
        });

    let node_config = NodeConfig::test()
        .dev()
        .with_rpc(RpcServerArgs::default().with_http())
        .with_chain(custom_chain());

    let add_ons: EthereumAddOns<_, _, _, _, PassthroughBlockExecutorValidatorBuilder> =
        EthereumAddOns::new(RpcAddOns::new(
            EthereumEthApiBuilder::<alloy_network::Ethereum>::default(),
            EthereumEngineValidatorBuilder::default(),
            BasicEngineApiBuilder::<EthereumEngineValidatorBuilder>::default(),
            PassthroughBlockExecutorValidatorBuilder {
                inner: BasicEngineValidatorBuilder::default(),
                custom_block_executor: passthrough_executor,
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

    println!("Node running with custom block executor hook — press Ctrl+C to exit");

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
