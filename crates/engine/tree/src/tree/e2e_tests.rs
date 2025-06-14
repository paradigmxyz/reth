//! E2E test implementations using the e2e test framework for engine tree functionality.

use crate::tree::TreeConfig;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{
        CaptureBlock, CreateFork, ExpectFcuStatus, MakeCanonical, ProduceBlocks,
        ProduceInvalidBlocks, ReorgTo, ValidateCanonicalTag,
    },
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
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
        .with_tree_config(
            TreeConfig::default().with_legacy_state_root(false).with_has_enough_parallelism(true),
        )
}

/// Test that verifies forkchoice update and canonical chain insertion functionality.
#[tokio::test]
async fn test_engine_tree_fcu_canon_chain_insertion_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // produce one block
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        // make it canonical via forkchoice update
        .with_action(MakeCanonical::new())
        // extend with 3 more blocks
        .with_action(ProduceBlocks::<EthEngineTypes>::new(3))
        // make the latest block canonical
        .with_action(MakeCanonical::new());

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test that verifies forkchoice update with a reorg where all blocks are already available.
#[tokio::test]
async fn test_engine_tree_fcu_reorg_with_all_blocks_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // create a main chain with 5 blocks (blocks 0-4)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(5))
        .with_action(MakeCanonical::new())
        // create a fork from block 2 with 3 additional blocks
        .with_action(CreateFork::<EthEngineTypes>::new(2, 3))
        .with_action(CaptureBlock::new("fork_tip"))
        // perform FCU to the fork tip - this should make the fork canonical
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("fork_tip"));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test that verifies valid forks with an older canonical head.
///
/// This test creates two competing fork chains starting from a common ancestor,
/// then switches between them using forkchoice updates, verifying that the engine
/// correctly handles chains where the canonical head is older than fork tips.
#[tokio::test]
async fn test_engine_tree_valid_forks_with_older_canonical_head_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // create base chain with 1 block (this will be our old head)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        .with_action(CaptureBlock::new("old_head"))
        .with_action(MakeCanonical::new())
        // extend base chain with 5 more blocks to establish a fork point
        .with_action(ProduceBlocks::<EthEngineTypes>::new(5))
        .with_action(CaptureBlock::new("fork_point"))
        .with_action(MakeCanonical::new())
        // revert to old head to simulate scenario where canonical head is older
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("old_head"))
        // create first competing chain (chain A) from fork point with 10 blocks
        .with_action(CreateFork::<EthEngineTypes>::new_from_tag("fork_point", 10))
        .with_action(CaptureBlock::new("chain_a_tip"))
        // create second competing chain (chain B) from same fork point with 10 blocks
        .with_action(CreateFork::<EthEngineTypes>::new_from_tag("fork_point", 10))
        .with_action(CaptureBlock::new("chain_b_tip"))
        // switch to chain B via forkchoice update - this should become canonical
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("chain_b_tip"));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test that verifies valid and invalid forks with an older canonical head.
#[tokio::test]
async fn test_engine_tree_valid_and_invalid_forks_with_older_canonical_head_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // create base chain with 1 block (old head)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        .with_action(CaptureBlock::new("old_head"))
        .with_action(MakeCanonical::new())
        // extend base chain with 5 more blocks to establish fork point
        .with_action(ProduceBlocks::<EthEngineTypes>::new(5))
        .with_action(CaptureBlock::new("fork_point"))
        .with_action(MakeCanonical::new())
        // revert to old head to simulate older canonical head scenario
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("old_head"))
        // create chain B (the valid chain) from fork point with 10 blocks
        .with_action(CreateFork::<EthEngineTypes>::new_from_tag("fork_point", 10))
        .with_action(CaptureBlock::new("chain_b_tip"))
        // make chain B canonical via FCU - this becomes the valid chain
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("chain_b_tip"))
        // create chain A (competing chain) - first produce valid blocks, then test invalid
        // scenario
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("fork_point"))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(10))
        .with_action(CaptureBlock::new("chain_a_tip"))
        // test that FCU to chain A tip returns VALID status (it's a valid competing chain)
        .with_action(ExpectFcuStatus::valid("chain_a_tip"))
        // attempt to produce invalid blocks (which should be rejected)
        .with_action(ProduceInvalidBlocks::<EthEngineTypes>::with_invalid_at(3, 2))
        // chain B remains the canonical chain
        .with_action(ValidateCanonicalTag::new("chain_b_tip"));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test that verifies engine tree behavior when handling invalid blocks.
/// This test demonstrates that invalid blocks are correctly rejected and that
/// attempts to build on top of them fail appropriately.
#[tokio::test]
async fn test_engine_tree_reorg_with_missing_ancestor_expecting_valid_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // build main chain (blocks 1-6)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(6))
        .with_action(MakeCanonical::new())
        .with_action(CaptureBlock::new("main_chain_tip"))
        // create a valid fork first
        .with_action(CreateFork::<EthEngineTypes>::new_from_tag("main_chain_tip", 5))
        .with_action(CaptureBlock::new("valid_fork_tip"))
        // FCU to the valid fork should work
        .with_action(ExpectFcuStatus::valid("valid_fork_tip"));

    test.run::<EthereumNode>().await?;

    // attempting to build invalid chains fails properly
    let invalid_test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        .with_action(ProduceBlocks::<EthEngineTypes>::new(3))
        .with_action(MakeCanonical::new())
        // This should fail when trying to build subsequent blocks on the invalid block
        .with_action(ProduceInvalidBlocks::<EthEngineTypes>::with_invalid_at(2, 0));

    assert!(invalid_test.run::<EthereumNode>().await.is_err());

    Ok(())
}
