use std::sync::Arc;

use crate::utils::{node, optimism_payload_attributes, setup};
use parking_lot::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node_config, _tasks, exec, wallet) = setup();
    let wallet = Arc::new(Mutex::new(wallet));

    let mut first_node = node(node_config.clone(), exec.clone()).await?;
    let mut second_node = node(node_config.clone(), exec.clone()).await?;

    // Make them peer
    first_node.network.add_peer(second_node.network.record()).await;
    second_node.network.add_peer(first_node.network.record()).await;

    // Make sure they establish a new session
    first_node.network.expect_session().await;
    second_node.network.expect_session().await;

    // Taken from optimism tests
    let tip: usize = 300;
    let canonical_chain = first_node
        .advance(
            tip as u64,
            || {
                let wallet = wallet.clone();
                Box::pin(async move { wallet.lock().optimism_l1_block_info_tx().await })
            },
            optimism_payload_attributes,
        )
        .await?;

    // only send forkchoice update to second node
    second_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip - 2]).await?;
    second_node.wait_block(tip as u64 - 1, canonical_chain[tip - 2], true).await?;

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    second_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip - 1]).await?;
    second_node.wait_block(tip as u64, canonical_chain[tip - 1], false).await?;

    Ok(())
}
