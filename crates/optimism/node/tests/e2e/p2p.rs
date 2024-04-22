use crate::utils::{advance_chain, node, setup};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node_config, _tasks, exec, wallet) = setup();
    let wallet = Arc::new(Mutex::new(wallet));

    let mut first_node = node(node_config.clone(), exec.clone(), 1).await?;
    let mut second_node = node(node_config.clone(), exec.clone(), 2).await?;
    let mut third_node = node(node_config.clone(), exec.clone(), 3).await?;

    // Make them peer
    first_node.connect(&mut second_node).await;
    second_node.connect(&mut third_node).await;
    third_node.connect(&mut first_node).await;

    let tip: usize = 300;
    let tip_index: usize = tip - 1;

    // On first node, create a chain up to block number 300a
    let canonical_payload_chain = advance_chain(tip, &mut first_node, wallet.clone()).await?;
    let canonical_chain =
        canonical_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On second node, sync optimistically up to block number 297a
    second_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip_index - 3]).await?;
    second_node.wait_block(tip as u64 - 3, canonical_chain[tip_index - 3], true).await?;

    // On third node, sync optimistically up to block number 300a
    third_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip_index]).await?;
    third_node.wait_block(tip as u64, canonical_chain[tip_index], true).await?;

    //  On second node, create a side chain: 298b ->  299b -> 300b
    wallet.lock().await.nonce -= 3;
    second_node.payload.timestamp = first_node.payload.timestamp - 3; // TODO: probably want to make it node agnostic
    let side_payload_chain = advance_chain(3, &mut second_node, wallet.clone()).await?;
    let side_chain = side_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On third node, cause a 3 block depth re-org
    assert!(side_chain[2] != canonical_chain[tip_index]);
    third_node.engine_api.update_optimistic_forkchoice(dbg!(side_chain[2])).await?;
    third_node.wait_block(side_payload_chain[2].0.block().number, side_chain[2], true).await?;


    Ok(())
}
