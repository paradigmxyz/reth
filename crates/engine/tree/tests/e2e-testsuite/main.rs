//! E2E test implementations using the e2e test framework for engine tree functionality.

use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{
        CaptureBlock, CompareNodeChainTips, CreateFork, ExpectFcuStatus, MakeCanonical,
        ProduceBlocks, ProduceBlocksLocally, ProduceInvalidBlocks, ReorgTo, SelectActiveNode,
        SendNewPayloads, UpdateBlockInfo, ValidateCanonicalTag, WaitForSync,
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

/// Test that verifies buffered blocks are eventually connected when sent in reverse order.
#[tokio::test]
async fn test_engine_tree_buffered_blocks_are_eventually_connected_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(
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
                .with_network(NetworkSetup::multi_node_unconnected(2)) // Need 2 disconnected nodes
                .with_tree_config(
                    TreeConfig::default()
                        .with_legacy_state_root(false)
                        .with_has_enough_parallelism(true),
                ),
        )
        // node 0 produces blocks 1 and 2 locally without broadcasting
        .with_action(SelectActiveNode::new(0))
        .with_action(ProduceBlocksLocally::<EthEngineTypes>::new(2))
        // make the blocks canonical on node 0 so they're available via RPC
        .with_action(MakeCanonical::with_active_node())
        // send blocks in reverse order (2, then 1) from node 0 to node 1
        .with_action(
            SendNewPayloads::<EthEngineTypes>::new()
                .with_target_node(1)
                .with_source_node(0)
                .with_start_block(1)
                .with_total_blocks(2)
                .in_reverse_order(),
        )
        // update node 1's view to recognize the new blocks
        .with_action(SelectActiveNode::new(1))
        // get the latest block from node 1's RPC and update environment
        .with_action(UpdateBlockInfo::default())
        // make block 2 canonical on node 1 with a forkchoice update
        .with_action(MakeCanonical::with_active_node())
        // verify both nodes eventually have the same chain tip
        .with_action(CompareNodeChainTips::expect_same(0, 1));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test that verifies forkchoice updates can extend the canonical chain progressively.
///
/// This test creates a longer chain of blocks, then uses forkchoice updates to make
/// different parts of the chain canonical in sequence, verifying that FCU properly
/// advances the canonical head when all blocks are already available.
#[tokio::test]
async fn test_engine_tree_fcu_extends_canon_chain_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let test = TestBuilder::new()
        .with_setup(default_engine_tree_setup())
        // create and make canonical a base chain with 1 block
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        .with_action(MakeCanonical::new())
        // extend the chain with 10 more blocks (total 11 blocks: 0-10)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(10))
        // capture block 6 as our intermediate target (from 0-indexed, this is block 6)
        .with_action(CaptureBlock::new("target_block"))
        // make the intermediate target canonical via FCU
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("target_block"))
        // now make the chain tip canonical via FCU
        .with_action(MakeCanonical::new());

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test that verifies live sync transition where a long chain eventually becomes canonical.
///
/// This test simulates a scenario where:
/// 1. Both nodes start with the same short base chain
/// 2. Node 0 builds a long chain locally (no broadcast, becomes its canonical tip)
/// 3. Node 1 still has only the short base chain as its canonical tip
/// 4. Node 1 receives FCU pointing to Node 0's long chain tip and must sync
/// 5. Both nodes end up with the same canonical chain through real P2P sync
#[tokio::test]
async fn test_engine_tree_live_sync_transition_eventually_canonical_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    const MIN_BLOCKS_FOR_PIPELINE_RUN: u64 = 32; // EPOCH_SLOTS from alloy-eips

    let test = TestBuilder::new()
        .with_setup(
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
                .with_network(NetworkSetup::multi_node(2)) // Two connected nodes
                .with_tree_config(
                    TreeConfig::default()
                        .with_legacy_state_root(false)
                        .with_has_enough_parallelism(true),
                ),
        )
        // Both nodes start with the same base chain (1 block)
        .with_action(SelectActiveNode::new(0))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        .with_action(MakeCanonical::new()) // Both nodes have the same base chain
        .with_action(CaptureBlock::new("base_chain_tip"))
        // Node 0: Build a much longer chain but don't broadcast it yet
        .with_action(ProduceBlocksLocally::<EthEngineTypes>::new(MIN_BLOCKS_FOR_PIPELINE_RUN + 10))
        .with_action(MakeCanonical::with_active_node()) // Only make it canonical on Node 0
        .with_action(CaptureBlock::new("long_chain_tip"))
        // Verify Node 0's canonical tip is the long chain tip
        .with_action(ValidateCanonicalTag::new("long_chain_tip"))
        // Verify Node 1's canonical tip is still the base chain tip
        .with_action(SelectActiveNode::new(1))
        .with_action(ValidateCanonicalTag::new("base_chain_tip"))
        // Node 1: Send FCU pointing to Node 0's long chain tip
        // This should trigger Node 1 to sync the missing blocks from Node 0
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("long_chain_tip"))
        // Wait for Node 1 to sync with Node 0
        .with_action(WaitForSync::new(0, 1).with_timeout(60))
        // Verify both nodes end up with the same canonical chain
        .with_action(CompareNodeChainTips::expect_same(0, 1));

    test.run::<EthereumNode>().await?;

    Ok(())
}
