use std::sync::{Arc, Mutex};

use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_e2e_test_utils::{node::NodeTestContext, wallet::Wallet};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpecBuilder, Genesis, MAINNET};
use reth_transaction_pool::TransactionPool;

use crate::utils::eth_payload_attributes;

#[tokio::test]
async fn can_handle_blobs() -> eyre::Result<()> {
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
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node).await?;

    let mut wallet = Wallet::default();
    let blob_tx = wallet.tx_with_blobs().await?;

    // inject blob tx to the pool
    let blob_tx_hash = node.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    let versioned_hashes = node.validate_sidecar(envelope);
    // build a payload with it
    let (blob_payload, attributes) = node.new_payload(eth_payload_attributes).await?;

    // clean the pool
    node.inner.pool.remove_transactions(vec![blob_tx_hash]);

    // inject normal tx
    let raw_tx = wallet.transfer_tx(None).await;
    node.inject_tx(raw_tx).await?;

    // build payload with normal tx
    let (payload, attributes) = node.new_payload(eth_payload_attributes).await?;

    // submit both
    let block_hash = node
        .engine_api
        .submit_payload(payload, attributes.clone(), versioned_hashes.clone())
        .await?;
    let blob_block_hash =
        node.engine_api.submit_payload(blob_payload, attributes.clone(), versioned_hashes).await?;

    // send fcu with blob hash
    node.engine_api.update_forkchoice(blob_block_hash).await?;
    // send fcu with normal hash
    node.engine_api.update_forkchoice(block_hash).await?;

    // expects the blob tx to be back in the pool
    let envelope = node.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    node.validate_sidecar(envelope);

    Ok(())
}
