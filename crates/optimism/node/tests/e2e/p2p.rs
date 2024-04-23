use crate::utils::{advance_chain, setup};
use reth_e2e_test_utils::{transaction::TransactionTestContext, wallet::Wallet};

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, _wallet) = setup(2).await?;

    let second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    let tip: usize = 300;
    let tip_index: usize = tip - 1;

    let wallet = Wallet::default();

    // On first node, create a chain up to block number 300a
    let canonical_payload_chain = advance_chain(tip, &mut first_node, || {
        let wallet = wallet.inner.clone();
        Box::pin(async move { TransactionTestContext::optimism_l1_block_info_tx(1, wallet).await })
    })
    .await?;
    let canonical_chain =
        canonical_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On second node, sync up to block number 300a
    second_node
        .engine_api
        .update_forkchoice(canonical_chain[tip_index], canonical_chain[tip_index])
        .await?;
    second_node.wait_block(tip as u64, canonical_chain[tip_index], true).await?;

    Ok(())
}
