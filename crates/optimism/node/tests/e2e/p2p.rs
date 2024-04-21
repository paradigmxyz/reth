use crate::utils::{advance_chain, node, setup};
use std::sync::Arc;
use tokio::sync::Mutex;

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

    let tip: usize = 300;
    let tip_index: usize = tip - 1;

    // Create a chain up to block number 300.
    let canonical_chain = advance_chain(tip, &mut first_node, wallet.clone()).await?;

    second_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip_index - 1]).await?;
    second_node.wait_block(tip as u64 - 1, canonical_chain[tip_index - 1], true).await?;

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    second_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip_index]).await?;
    second_node.wait_block(tip as u64, canonical_chain[tip_index], false).await?;

    Ok(())
}
