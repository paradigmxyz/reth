use std::sync::Arc;

use alloy_genesis::Genesis;
use alloy_primitives::b256;
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    rpc::types::engine::PayloadStatusEnum,
    tasks::TaskManager,
};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_ethereum::EthereumNode;
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

    let wallets = Wallet::new(2).gen();
    let blob_wallet = wallets.first().unwrap();
    let second_wallet = wallets.last().unwrap();

    // inject normal tx
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, second_wallet.clone()).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    // build payload with normal tx
    let (payload, attributes) = node.new_payload(eth_payload_attributes).await?;

    // clean the pool
    node.inner.pool.remove_transactions(vec![tx_hash]);

    // build blob tx
    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, blob_wallet.clone()).await?;

    // inject blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    let versioned_hashes = TransactionTestContext::validate_sidecar(envelope);

    // build a payload
    let (blob_payload, blob_attr) = node.new_payload(eth_payload_attributes).await?;

    // submit the blob payload
    let blob_block_hash = node
        .engine_api
        .submit_payload(blob_payload, blob_attr, PayloadStatusEnum::Valid, versioned_hashes.clone())
        .await?;

    let genesis_hash = b256!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3");

    let (_, _) = tokio::join!(
        // send fcu with blob hash
        node.engine_api.update_forkchoice(genesis_hash, blob_block_hash),
        // send fcu with normal hash
        node.engine_api.update_forkchoice(genesis_hash, payload.block().hash())
    );

    // submit normal payload
    node.engine_api.submit_payload(payload, attributes, PayloadStatusEnum::Valid, vec![]).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // expects the blob tx to be back in the pool
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // make sure the sidecar is present
    TransactionTestContext::validate_sidecar(envelope);

    Ok(())
}
