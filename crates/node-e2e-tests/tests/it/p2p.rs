use std::sync::Arc;

use node_e2e_tests::{node::NodeHelper, wallet::Wallet};
use reth::tasks::TaskManager;
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let wallet = Wallet::default();
    let raw_tx = wallet.transfer_tx().await;

    let mut first_node = NodeHelper::new(chain_spec.clone(), exec.clone()).await?;
    let mut second_node = NodeHelper::new(chain_spec, exec).await?;

    // Make them peer
    first_node.add_peer(second_node.record()).await;
    second_node.add_peer(first_node.record()).await;

    // Make sure they establish a new session
    first_node.expect_session().await;
    second_node.expect_session().await;

    // Make the first node advance
    let (block_hash, tx_hash) = first_node.advance(raw_tx.clone()).await?;

    // only send forkchoice update to second node
    second_node.update_forkchoice(block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash).await?;

    Ok(())
}
