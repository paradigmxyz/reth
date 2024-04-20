use std::sync::Arc;

use reth::{
    args::{DiscoveryArgs, NetworkArgs, RpcServerArgs},
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_e2e_test_utils::{node::NodeTestContext, wallet::Wallet};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};

use crate::utils::eth_payload_attributes;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let network_config = NetworkArgs {
        discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
        ..NetworkArgs::default()
    };

    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_network(network_config)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut first_node = NodeTestContext::new(node.clone()).await?;

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(exec)
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut second_node = NodeTestContext::new(node).await?;

    let mut wallet = Wallet::default();
    let raw_tx = wallet.transfer_tx().await;

    // Make them peer
    first_node.network.add_peer(second_node.network.record()).await;
    second_node.network.add_peer(first_node.network.record()).await;

    // Make sure they establish a new session
    first_node.network.expect_session().await;
    second_node.network.expect_session().await;

    let tx_hash = first_node.inject_tx(raw_tx).await?;

    // Make the first node advance
    let (block_hash, block_number) = first_node.advance(vec![], eth_payload_attributes).await?;

    // assert the block has been committed to the blockchain
    first_node.assert_new_block(tx_hash, block_hash, block_number).await?;

    // only send forkchoice update to second node
    second_node.engine_api.update_forkchoice(block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}
