use std::sync::Arc;

use node_e2e_tests::{node::NodeTestContext, wallet::Wallet};
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};

#[tokio::test]
async fn can_handle_blobs() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis =
        serde_json::from_str(include_str!("../../../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node.clone()).await?;

    let wallet = Wallet::default();
    let raw_tx = wallet.tx_with_blobs().await?;

    // inject blob tx to the pool
    let tx_hash = node.inject_tx(raw_tx).await?;
    // fetch it from rpc
    let envelope = node.envelope_by_hash(tx_hash).await?;
    // validate sidecar
    let versioned_hashes = node.validate_sidecar(envelope);

    // advance the node
    let block_hash = node.advance(versioned_hashes).await?;

    // assert the block has been committed to the blockchain
    node.assert_new_block(tx_hash, block_hash).await?;

    // fetch the tx from rpc
    let envelope = node.envelope_by_hash(tx_hash).await?;
    // validate sidecar
    node.validate_sidecar(envelope); // Currently failing as its returning a TxEip4844 and not a TxEip4844WithSidecar

    Ok(())
}
