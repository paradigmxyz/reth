use crate::utils::{advance_chain, setup};
use reth::primitives::BASE_MAINNET;
use reth_e2e_test_utils::{transaction::TransactionTestContext, wallet::Wallet};
use reth_primitives::ChainId;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, wallet) = setup(3).await?;
    let wallet = Arc::new(Mutex::new(wallet));

    let mut third_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

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
    wallet.lock().await.inner_nonce -= 3;
    second_node.payload.timestamp = first_node.payload.timestamp - 3; // TODO: probably want to make it node agnostic
    let side_payload_chain = advance_chain(3, &mut second_node, wallet.clone()).await?;
    let side_chain = side_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On third node, cause a 3 block depth re-org
    assert!(side_chain[2] != canonical_chain[tip_index]);
    third_node.engine_api.update_optimistic_forkchoice(dbg!(side_chain[2])).await?;
    third_node.wait_block(side_payload_chain[2].0.block().number, side_chain[2], true).await?;

    Ok(())
}
