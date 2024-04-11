use futures_util::StreamExt;
use node_e2e_tests::{
    node::{eth_payload_attributes, NodeHelper},
    wallet::Wallet,
};
use reth::{self, tasks::TaskManager};
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};
use std::sync::Arc;

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let exec = TaskManager::current();
    let exec = exec.executor();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    // Configure wallet from test mnemonic and create dummy transfer tx
    let wallet = Wallet::default();
    let raw_tx = wallet.transfer_tx().await;

    // Node setup
    let mut node = NodeHelper::new(chain_spec, exec).await?;

    // make the node advance
    node.advance(raw_tx).await?;

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn can_run_eth_node_with_auth_engine_api_over_ipc() -> eyre::Result<()> {
    use reth::{
        args::RpcServerArgs,
        builder::{NodeBuilder, NodeConfig, NodeHandle},
        providers::CanonStateSubscriptions,
    };
    use reth_node_ethereum::EthereumNode;
    use reth_rpc::eth::EthTransactions;

    reth_tracing::init_test_tracing();
    let exec = TaskManager::current();
    let exec = exec.executor();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../../assets/genesis.json")).unwrap();
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

    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(exec)
        .node(EthereumNode::default())
        .launch()
        .await?;

    // setup engine api events and payload service events
    let _notifications = node.provider.canonical_state_stream();
    let payload_events = node.payload_builder.subscribe().await?;
    let mut payload_event_stream = payload_events.into_stream();

    // push tx into pool via RPC server
    let eth_api = node.rpc_registry.eth_api();
    let wallet = Wallet::default();
    let raw_tx = wallet.transfer_tx().await;

    eth_api.send_raw_transaction(raw_tx).await?;

    // trigger new payload building draining the pool
    let eth_attr = eth_payload_attributes();
    let _payload_id = node.payload_builder.new_payload(eth_attr.clone()).await?;

    // first event is the payload attributes
    let first_event = payload_event_stream.next().await.unwrap()?;
    if let reth::payload::Events::Attributes(attr) = first_event {
        assert_eq!(eth_attr.timestamp, attr.timestamp);
    } else {
        panic!("Expect first event as payload attributes.")
    }

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn test_failed_run_eth_node_with_no_auth_engine_api_over_ipc_opts() -> eyre::Result<()> {
    use reth::builder::{NodeBuilder, NodeConfig, NodeHandle};
    use reth_node_ethereum::EthereumNode;

    reth_tracing::init_test_tracing();
    let exec = TaskManager::current();
    let exec = exec.executor();
    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../../assets/genesis.json")).unwrap();
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

    let client = node.engine_ipc_client().await;
    assert!(client.is_none(), "ipc auth should be disabled by default");

    Ok(())
}
