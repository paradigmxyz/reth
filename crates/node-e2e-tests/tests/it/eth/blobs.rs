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

    let mut wallet = Wallet::default();
    let blob_tx = wallet.tx_with_blobs().await?;

    // inject blob tx to the pool
    let blob_tx_hash = node.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    let versioned_hashes = node.validate_sidecar(envelope);
    // advance the node with the blob tx
    let blob_tx_block_hash = node.advance(versioned_hashes).await?;

    let tx = wallet.transfer_tx().await;
    let tx_hash = node.inject_tx(tx).await?;
    let tx_block_hash = node.advance(vec![]).await?;

    let envelope = node.envelope_by_hash(blob_tx_hash).await?;

    node.validate_sidecar(envelope);
    // build block with blob tx
    // build block without blob tx
    // send both
    // send FCU with the blob tx
    // send FCU without the blob tx
    // expect the blob tx to be in the pool again and sidecar should be valid

    Ok(())
}
