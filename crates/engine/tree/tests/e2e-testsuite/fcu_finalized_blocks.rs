//! E2E tests for forkchoice updates to canonical ancestors around the finalized block.

use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{
        AssertChainTip, BlockReference, CaptureBlock, CreateFork, FinalizeBlock, MakeCanonical,
        ProduceBlocks, SendForkchoiceUpdate, UpdateBlockInfo,
    },
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_engine_tree::tree::TreeConfig;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

/// Creates the standard setup for engine tree e2e tests.
fn default_engine_tree_setup() -> Setup<EthEngineTypes> {
    Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(
                    serde_json::from_str(include_str!(
                        "../../../../e2e-test-utils/src/testsuite/assets/genesis.json"
                    ))
                    .unwrap(),
                )
                .cancun_activated()
                .build(),
        ))
        .with_network(NetworkSetup::single_node())
        .with_tree_config(TreeConfig::default().with_has_enough_parallelism(true))
}

/// Verifies that an FCU to a canonical ancestor above finality can start a payload build on top
/// of the ancestor whose block eventually reorgs out the current head.
#[tokio::test]
async fn test_fcu_to_canonical_ancestor_around_finalized() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // Build and tag canonical ancestors on the way to block 10.
        .with_action(ProduceBlocks::<EthEngineTypes>::new(7))
        .with_action(CaptureBlock::new("block_7"))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        .with_action(CaptureBlock::new("block_8"))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(2))
        .with_action(CaptureBlock::new("block_10"))
        .with_action(MakeCanonical::new())
        // Establish block 7 as the latest known finalized block.
        .with_action(
            FinalizeBlock::<EthEngineTypes>::new(BlockReference::Tag("block_7".to_string()))
                .with_head(BlockReference::Tag("block_10".to_string())),
        )
        // Block 8 is above finality: without payload attributes the FCU is acknowledged, but the
        // canonical head does not move.
        .with_action(
            SendForkchoiceUpdate::<EthEngineTypes>::new(
                BlockReference::Tag("block_7".to_string()),
                BlockReference::Tag("block_7".to_string()),
                BlockReference::Tag("block_8".to_string()),
            )
            .with_expected_status(alloy_rpc_types_engine::PayloadStatusEnum::Valid),
        )
        .with_action(UpdateBlockInfo::default())
        .with_action(AssertChainTip::new(10))
        // With payload attributes, an FCU to block 8 starts a payload build on top of it:
        // `CreateFork` drives the full fcu(attrs) -> getPayload -> newPayload flow. Making the
        // built block canonical reorgs out the previous blocks 9 and 10.
        .with_action(CreateFork::<EthEngineTypes>::new_from_tag("block_8", 1))
        .with_action(MakeCanonical::new())
        .with_action(AssertChainTip::new(9));

    test.run::<EthereumNode>().await?;

    Ok(())
}
