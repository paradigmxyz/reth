//! E2E tests for forkchoice updates to canonical ancestors around the finalized block.

use alloy_primitives::B256;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{
        AssertChainTip, BlockReference, CaptureBlock, FinalizeBlock, MakeCanonical, ProduceBlocks,
        SendForkchoiceUpdate, UpdateBlockInfo,
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

/// Verifies that an FCU to a canonical ancestor at or below the latest known finalized block is
/// skipped, while an FCU to an ancestor above finality updates the canonical head.
#[tokio::test]
async fn test_fcu_to_canonical_ancestor_around_finalized() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // Build and tag canonical ancestors on the way to block 10.
        .with_action(ProduceBlocks::<EthEngineTypes>::new(5))
        .with_action(CaptureBlock::new("block_5"))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(2))
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
        // The finalized-prefix optimization uses stored finality, even when the new FCU carries
        // zero hashes. Block 5 is acknowledged without moving the canonical head from block 10.
        .with_action(
            SendForkchoiceUpdate::<EthEngineTypes>::new(
                BlockReference::Hash(B256::ZERO),
                BlockReference::Hash(B256::ZERO),
                BlockReference::Tag("block_5".to_string()),
            )
            .with_expected_status(alloy_rpc_types_engine::PayloadStatusEnum::Valid),
        )
        .with_action(UpdateBlockInfo::default())
        .with_action(AssertChainTip::new(10))
        // Block 8 is above finality, so the FCU must rewind the canonical head to it.
        .with_action(
            SendForkchoiceUpdate::<EthEngineTypes>::new(
                BlockReference::Tag("block_7".to_string()),
                BlockReference::Tag("block_7".to_string()),
                BlockReference::Tag("block_8".to_string()),
            )
            .with_expected_status(alloy_rpc_types_engine::PayloadStatusEnum::Valid),
        )
        .with_action(UpdateBlockInfo::default())
        .with_action(AssertChainTip::new(8));

    test.run::<EthereumNode>().await?;

    Ok(())
}
