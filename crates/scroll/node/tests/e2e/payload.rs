use reth_scroll_node::test_utils::{advance_chain, setup};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, _tasks, wallet) = setup(1, false).await?;
    let mut node = node.pop().unwrap();
    let wallet = Arc::new(Mutex::new(wallet));

    let tip: usize = 90;

    // Create a chain of 90 blocks
    let _canonical_payload_chain = advance_chain(tip, &mut node, wallet.clone()).await?;

    Ok(())
}
