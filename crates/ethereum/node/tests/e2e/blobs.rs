use crate::utils::eth_payload_attributes;
use alloy_genesis::Genesis;
use alloy_rpc_types_engine::PayloadStatusEnum;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_tasks::TaskManager;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

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
    let genesis_hash = chain_spec.genesis_hash();
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallets = Wallet::new(2).gen();
    let blob_wallet = wallets.first().unwrap();
    let second_wallet = wallets.last().unwrap();

    // inject normal tx
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, second_wallet.clone()).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    // build payload with normal tx
    let (payload, attributes) = node.new_payload().await?;

    // clean the pool
    node.inner.pool.remove_transactions(vec![tx_hash]);

    // build blob tx
    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, blob_wallet.clone()).await?;

    // inject blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // build a payload
    let (blob_payload, blob_attr) = node.new_payload().await?;

    // submit the blob payload
    let blob_block_hash =
        node.engine_api.submit_payload(blob_payload, blob_attr, PayloadStatusEnum::Valid).await?;

    node.engine_api.update_forkchoice(genesis_hash, blob_block_hash).await?;

    // submit normal payload (reorg)
    let block_hash =
        node.engine_api.submit_payload(payload, attributes, PayloadStatusEnum::Valid).await?;
    node.engine_api.update_forkchoice(genesis_hash, block_hash).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // expects the blob tx to be back in the pool
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // make sure the sidecar is present
    TransactionTestContext::validate_sidecar(envelope);

    Ok(())
}
