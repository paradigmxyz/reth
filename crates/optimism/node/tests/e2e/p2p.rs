use crate::utils::{advance_chain, setup};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, _, wallet) = setup(2).await?;
    let wallet = Arc::new(Mutex::new(wallet));

    let second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    let tip: usize = 300;
    let tip_index: usize = tip - 1;

    // On first node, create a chain up to block number 300a
    let canonical_payload_chain = advance_chain(tip, &mut first_node, wallet.clone()).await?;
    let canonical_chain =
        canonical_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On second node, sync up to block number 300a
    second_node.engine_api.update_forkchoice(canonical_chain[tip_index]).await?;
    second_node.wait_block(tip as u64, canonical_chain[tip_index], true).await?;

    Ok(())
}
