use crate::utils::eth_payload_attributes;
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_e2e_test_utils::{
    node::NodeTestContext, setup, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};
use std::sync::Arc;

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, _wallet) = setup::<EthereumNode>(
        1,
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

    let mut node = nodes.pop().unwrap();
    let wallet = Wallet::default();
    let raw_tx = TransactionTestContext::transfer_tx(1, wallet.inner).await;

    // make the node advance
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let (payload, _) = node.advance_block(vec![], eth_payload_attributes).await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    node.assert_new_block(tx_hash, block_hash, block_number).await?;

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn can_run_eth_node_with_auth_engine_api_over_ipc() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let exec = TaskManager::current();
    let exec = exec.executor();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    // Node setup
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http().with_auth_ipc());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(exec)
        .node(EthereumNode::default())
        .launch()
        .await?;
    let mut node = NodeTestContext::new(node).await?;

    // Configure wallet from test mnemonic and create dummy transfer tx
    let wallet = Wallet::default();
    let raw_tx = TransactionTestContext::transfer_tx(1, wallet.inner).await;

    // make the node advance
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let (payload, _) = node.advance_block(vec![], eth_payload_attributes).await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    node.assert_new_block(tx_hash, block_hash, block_number).await?;

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn test_failed_run_eth_node_with_no_auth_engine_api_over_ipc_opts() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let exec = TaskManager::current();
    let exec = exec.executor();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    // Node setup
    let node_config = NodeConfig::test().with_chain(chain_spec);
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(exec)
        .node(EthereumNode::default())
        .launch()
        .await?;

    let node = NodeTestContext::new(node).await?;

    // Ensure that the engine api client is not available
    let client = node.inner.engine_ipc_client().await;
    assert!(client.is_none(), "ipc auth should be disabled by default");

    Ok(())
}
