//! Example: Running a reth node with a custom state root strategy.
//!
//! This demonstrates how to use the [`CustomStateRoot`] hook to override state
//! root computation. The example always returns `B256::ZERO` as the state root.
//!
//! The key integration point is wrapping [`BasicEngineValidatorBuilder`] to call
//! [`BasicEngineValidator::with_custom_state_root`] on the resulting validator.
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
use reth_engine_tree::tree::{payload_validator::CustomStateRoot, BasicEngineValidator};
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
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
use reth_trie::updates::TrieUpdates;

// ---------------------------------------------------------------------------
// Custom Engine Validator Builder
// ---------------------------------------------------------------------------

/// An [`EngineValidatorBuilder`] that wraps [`BasicEngineValidatorBuilder`] and
/// installs a [`CustomStateRoot`] on the resulting [`BasicEngineValidator`].
#[derive(Clone)]
struct ZeroStateRootValidatorBuilder {
    inner: BasicEngineValidatorBuilder<EthereumEngineValidatorBuilder>,
    custom_state_root: CustomStateRoot<EthPrimitives>,
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
        Ok(validator.with_custom_state_root(self.custom_state_root))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let runtime = Runtime::test();

    // A custom state root handler that always returns B256::ZERO.
    let zero_state_root: CustomStateRoot<EthPrimitives> =
        Arc::new(|_input| Ok((B256::ZERO, TrieUpdates::default())));

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
                custom_state_root: zero_state_root,
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
