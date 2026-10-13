//! Tests for the finality policy of blocks imported by the test context.

use alloy_primitives::B256;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{node::Finality, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;

/// Tests that imported blocks become the head, while the safe and finalized blocks follow the
/// finality policy of the test context.
#[tokio::test]
async fn imported_blocks_follow_finality_policy() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, _) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let genesis = node.block_hash(0);
    let state = |head: B256, finalized: B256| ForkchoiceState {
        head_block_hash: head,
        safe_block_hash: finalized,
        finalized_block_hash: finalized,
    };

    node.set_finality(Finality::Keep);
    for _ in 0..2 {
        let head = node.advance_block().await?.block().hash();
        assert_eq!(node.current_forkchoice_state()?, state(head, genesis));
    }

    // Block 3 lags to genesis, which is finalized already, block 4 lags to block 1.
    node.set_finality(Finality::Lag(3));
    let head = node.advance_block().await?.block().hash();
    assert_eq!(node.current_forkchoice_state()?, state(head, genesis));
    let head = node.advance_block().await?.block().hash();
    assert_eq!(node.current_forkchoice_state()?, state(head, node.block_hash(1)));

    node.set_finality(Finality::Head);
    let finalized = node.advance_block().await?.block().hash();
    assert_eq!(node.current_forkchoice_state()?, state(finalized, finalized));

    // Block 6 lags to block 4, which is below the finalized block 5.
    node.set_finality(Finality::Lag(2));
    let head = node.advance_block().await?.block().hash();
    assert_eq!(node.current_forkchoice_state()?, state(head, finalized));

    Ok(())
}
