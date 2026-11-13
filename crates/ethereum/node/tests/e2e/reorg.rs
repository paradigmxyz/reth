//! Tests for building forks and reorging with the test context.

use alloy_primitives::B256;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{node::Finality, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;

/// Tests that a fork built with `advance_fork` becomes the head, that `reorg_to` switches back to
/// the previous chain, and that both explain why they can't reorg below the finalized block.
#[tokio::test]
async fn can_build_forks_and_reorg() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, _) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let genesis = node.block_hash(0);
    node.set_finality(Finality::Keep);

    // a1 <- a2 and b1 <- b2 <- b3, both on genesis.
    let a = node.advance_blocks(2).await?;
    let (a1, a2) = (a[0].block().hash(), a[1].block().hash());
    let b = node.advance_fork(genesis, 3).await?;
    let (b1, b3) = (b[0].block().hash(), b[2].block().hash());
    assert_eq!(b[0].block().parent_hash, genesis);
    assert_eq!(
        node.current_forkchoice_state()?,
        ForkchoiceState {
            head_block_hash: b3,
            safe_block_hash: genesis,
            finalized_block_hash: genesis
        }
    );

    // Building on a canonical ancestor leaves the head.
    assert_eq!(node.build_payload_on(b1).await?.block().parent_hash, b1);
    assert_eq!(node.current_forkchoice_state()?.head_block_hash, b3);

    node.reorg_to(a2).await?;
    assert_eq!(node.current_forkchoice_state()?.head_block_hash, a2);

    assert_eq!(
        node.reorg_to(a1).await.unwrap_err().to_string(),
        format!(
            "block {a1} is a canonical ancestor of the head {a2}, which the engine does not move \
             the head back to, build a block on it with `advance_block_on` instead"
        )
    );

    // Once a3 is finalized, neither chain B nor the blocks below a3 can become the head.
    node.set_finality(Finality::Head);
    let a3 = node.advance_block().await?.block().hash();
    let below_finalized = |head: B256| {
        format!(
            "the node can't reorg to block {head}: it does not descend from the safe and \
             finalized blocks of the node (finalized: 3 ({a3})). Only blocks above the finalized \
             block can be reorged, import them under `Finality::Keep` or `Finality::Lag`, see \
             `NodeTestContext::set_finality`"
        )
    };
    assert_eq!(node.reorg_to(b3).await.unwrap_err().to_string(), below_finalized(b3));
    assert_eq!(node.build_payload_on(a2).await.unwrap_err().to_string(), below_finalized(a2));

    let unknown = B256::repeat_byte(1);
    assert_eq!(
        node.reorg_to(unknown).await.unwrap_err().to_string(),
        format!(
            "the node does not know block {unknown} or one of its ancestors, or is syncing, \
             submit the blocks first, e.g. with `submit_payload`"
        )
    );
    assert_eq!(node.current_forkchoice_state()?, ForkchoiceState::same_hash(a3));

    Ok(())
}
