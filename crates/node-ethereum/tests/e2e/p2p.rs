use crate::utils::eth_payload_attributes;
use reth_e2e_test_utils::setup;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpecBuilder, MAINNET};
use std::sync::Arc;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, mut wallet) = setup::<EthereumNode>(
        2,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ),
        false,
    )
    .await?;

    let raw_tx = wallet.transfer_tx().await;
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    // Make the first node advance
    let ((payload, _), tx_hash) =
        first_node.advance_block(raw_tx.clone(), eth_payload_attributes).await?;
    let block_hash = payload.block().hash();

    // only send forkchoice update to second node
    second_node.engine_api.update_forkchoice(block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}
