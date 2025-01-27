use alloy_rpc_types_engine::PayloadStatusEnum;
use futures::StreamExt;
use reth_optimism_node::utils::{advance_chain, setup};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, wallet) = setup(3).await?;
    let wallet = Arc::new(Mutex::new(wallet));

    let third_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    let tip: usize = 90;
    let tip_index: usize = tip - 1;
    let reorg_depth = 2;

    // On first node, create a chain up to block number 90a
    let canonical_payload_chain = advance_chain(tip, &mut first_node, wallet.clone()).await?;
    let canonical_chain =
        canonical_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On second node, sync optimistically up to block number 88a
    second_node
        .engine_api
        .update_optimistic_forkchoice(canonical_chain[tip_index - reorg_depth - 1])
        .await?;
    second_node
        .wait_block(
            (tip - reorg_depth - 1) as u64,
            canonical_chain[tip_index - reorg_depth - 1],
            true,
        )
        .await?;
    // We send FCU twice to ensure that pool receives canonical chain update on the second FCU
    // This is required because notifications are not sent during backfill sync
    second_node
        .engine_api
        .update_optimistic_forkchoice(canonical_chain[tip_index - reorg_depth])
        .await?;
    second_node
        .wait_block((tip - reorg_depth) as u64, canonical_chain[tip_index - reorg_depth], false)
        .await?;
    second_node.engine_api.canonical_stream.next().await.unwrap();

    // Trigger backfil sync until block 80
    third_node
        .engine_api
        .update_forkchoice(canonical_chain[tip_index - 10], canonical_chain[tip_index - 10])
        .await?;
    third_node.wait_block((tip - 10) as u64, canonical_chain[tip_index - 10], true).await?;
    // Trigger live sync to block 90
    third_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip_index]).await?;
    third_node.wait_block(tip as u64, canonical_chain[tip_index], false).await?;

    //  On second node, create a side chain: 88a -> 89b -> 90b
    wallet.lock().await.inner_nonce -= reorg_depth as u64;
    second_node.payload.timestamp = first_node.payload.timestamp - reorg_depth as u64; // TODO: probably want to make it node agnostic
    let side_payload_chain = advance_chain(reorg_depth, &mut second_node, wallet.clone()).await?;
    let side_chain = side_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // Creates fork chain by submitting 89b payload.
    // By returning Valid here, op-node will finally return a finalized hash
    let _ = third_node
        .engine_api
        .submit_payload(
            side_payload_chain[0].0.clone(),
            side_payload_chain[0].1.clone(),
            PayloadStatusEnum::Valid,
        )
        .await;

    // It will issue a pipeline reorg to 88a, and then make 89b canonical AND finalized.
    third_node.engine_api.update_forkchoice(side_chain[0], side_chain[0]).await?;

    // Make sure we have the updated block
    third_node.wait_unwind((tip - reorg_depth) as u64).await?;
    third_node
        .wait_block(
            side_payload_chain[0].0.block().number,
            side_payload_chain[0].0.block().hash(),
            false,
        )
        .await?;

    Ok(())
}
