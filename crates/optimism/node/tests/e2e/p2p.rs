use std::sync::Arc;

use crate::utils::optimism_payload_attributes;
use reth::{
    args::{DiscoveryArgs, NetworkArgs, RpcServerArgs},
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_e2e_test_utils::{node::NodeTestContext, wallet::Wallet};
use reth_node_optimism::node::OptimismNode;
use reth_primitives::{hex, Bytes, ChainSpecBuilder, Genesis, BASE_MAINNET};

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(BASE_MAINNET.chain)
            .genesis(genesis)
            .ecotone_activated()
            .build(),
    );
    let mut wallet = Wallet::default().with_chain_id(chain_spec.chain.into());

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
        .node(OptimismNode::default())
        .launch()
        .await?;

    let mut first_node = NodeTestContext::new(node.clone()).await?;

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(exec)
        .node(OptimismNode::default())
        .launch()
        .await?;

    let mut second_node = NodeTestContext::new(node).await?;

    // Make them peer
    first_node.network.add_peer(second_node.network.record()).await;
    second_node.network.add_peer(first_node.network.record()).await;

    // Make sure they establish a new session
    first_node.network.expect_session().await;
    second_node.network.expect_session().await;

    // Taken from optimism tests
    let l1_block_info = Bytes::from_static(&hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240"));

    // Make the first node advance
    let raw_tx = wallet.transfer_tx(Some(l1_block_info)).await;
    let tx_hash = first_node.rpc.inject_tx(raw_tx).await?;
    let (block_hash, block_number) =
        first_node.advance(vec![], optimism_payload_attributes).await?;

    // only send forkchoice update to second node
    second_node.engine_api.update_forkchoice(block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, block_number).await?;

    Ok(())
}
