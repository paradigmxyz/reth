use crate::utils::eth_payload_attributes;
use reth::tasks::TaskManager;
use reth_e2e_test_utils::TestNetworkBuilder;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};
use std::sync::Arc;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json"))?;
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let mut nodes = TestNetworkBuilder::<EthereumNode>::new(2, chain_spec, exec).build().await?;

    let mut first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();

    let raw_tx = first_node.wallets.first().unwrap().eip1559().await;

    // make the first node advance
    let (payload, _, tx_hash) = first_node.advance(vec![], eth_payload_attributes, raw_tx).await?;

    let block_hash = payload.block().hash();

    // only send forkchoice update to second node
    second_node.engine_api.update_forkchoice(block_hash, block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}
