use crate::utils::eth_payload_attributes;
use alloy_eips::Decodable2718;
use alloy_genesis::Genesis;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_ethereum_engine_primitives::BlobSidecars;
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_tasks::Runtime;
use reth_transaction_pool::TransactionPool;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::test]
async fn can_handle_blobs() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

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
        .testing_node(runtime.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallets = Wallet::new(2).wallet_gen();
    let blob_wallet = wallets.first().unwrap();
    let second_wallet = wallets.last().unwrap();

    // inject normal tx
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, second_wallet.clone()).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    // build payload with normal tx
    let payload = node.new_payload().await?;

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
    let blob_payload = node.new_payload().await?;

    // submit the blob payload
    let blob_block_hash = node.submit_payload(blob_payload).await?;

    node.update_forkchoice(genesis_hash, blob_block_hash).await?;

    // submit normal payload (reorg)
    let block_hash = node.submit_payload(payload).await?;
    node.update_forkchoice(genesis_hash, block_hash).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // expects the blob tx to be back in the pool
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // make sure the sidecar is present
    TransactionTestContext::validate_sidecar(envelope);

    Ok(())
}

#[tokio::test]
async fn can_send_legacy_sidecar_post_activation() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default().chain(MAINNET.chain).genesis(genesis).osaka_activated().build(),
    );
    let genesis_hash = chain_spec.genesis_hash();
    let node_config = NodeConfig::test().with_chain(chain_spec).with_unused_ports().with_rpc(
        RpcServerArgs::default()
            .with_unused_ports()
            .with_http()
            .with_force_blob_sidecar_upcasting(),
    );
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(runtime.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallets = Wallet::new(2).wallet_gen();
    let blob_wallet = wallets.first().unwrap();

    // build blob tx
    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, blob_wallet.clone()).await?;

    let tx = PooledTransactionVariant::decode_2718_exact(&blob_tx).unwrap();
    assert!(tx.as_eip4844().unwrap().tx().sidecar.is_eip4844());

    // inject blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // assert that sidecar was converted to eip7594 (force upcasting is enabled)
    assert!(envelope.as_eip4844().unwrap().tx().sidecar().unwrap().is_eip7594());
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // build a payload
    let blob_payload = node.new_payload().await?;

    // submit the blob payload
    let blob_block_hash = node.submit_payload(blob_payload).await?;

    node.update_forkchoice(genesis_hash, blob_block_hash).await?;

    Ok(())
}

#[tokio::test]
async fn blob_conversion_at_osaka() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

    let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    // Osaka activates in 2 slots
    let osaka_timestamp = current_timestamp + 24;

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .prague_activated()
            .with_osaka_at(osaka_timestamp)
            .build(),
    );
    let genesis_hash = chain_spec.genesis_hash();
    let node_config = NodeConfig::test().with_chain(chain_spec).with_unused_ports().with_rpc(
        RpcServerArgs::default()
            .with_unused_ports()
            .with_http()
            .with_force_blob_sidecar_upcasting(),
    );
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(runtime.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let mut wallets = Wallet::new(3).wallet_gen();
    let first = wallets.pop().unwrap();
    let second = wallets.pop().unwrap();

    // build a dummy payload at `current_timestamp`
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallets.pop().unwrap()).await;
    node.rpc.inject_tx(raw_tx).await?;
    node.payload.timestamp = current_timestamp - 1;
    node.advance_block().await?;

    // build blob txs
    let first_blob = TransactionTestContext::tx_with_blobs_bytes(1, first.clone()).await?;
    let second_blob = TransactionTestContext::tx_with_blobs_bytes(1, second.clone()).await?;

    // assert both txs have legacy sidecars
    assert!(PooledTransactionVariant::decode_2718_exact(&first_blob)
        .unwrap()
        .as_eip4844()
        .unwrap()
        .tx()
        .sidecar
        .is_eip4844());
    assert!(PooledTransactionVariant::decode_2718_exact(&second_blob)
        .unwrap()
        .as_eip4844()
        .unwrap()
        .tx()
        .sidecar
        .is_eip4844());

    // inject first blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(first_blob).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // assert that it still has a legacy sidecar
    assert!(envelope.as_eip4844().unwrap().tx().sidecar().unwrap().is_eip4844());
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // build last Prague payload
    node.payload.timestamp = current_timestamp + 1;
    let prague_payload = node.new_payload().await?;
    assert!(matches!(prague_payload.sidecars(), BlobSidecars::Eip4844(_)));

    // inject second blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(second_blob).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // assert that it still has a legacy sidecar
    assert!(envelope.as_eip4844().unwrap().tx().sidecar().unwrap().is_eip4844());
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    tokio::time::sleep(Duration::from_secs(6)).await;

    // fetch second blob tx from rpc again
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // assert that it was converted to eip7594
    assert!(envelope.as_eip4844().unwrap().tx().sidecar().unwrap().is_eip7594());
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // submit the Prague payload
    node.update_forkchoice(genesis_hash, node.submit_payload(prague_payload).await?).await?;

    // Build first Osaka payload
    node.payload.timestamp = osaka_timestamp - 1;
    let osaka_payload = node.new_payload().await?;

    // Assert that it includes the second blob tx with eip7594 sidecar
    assert!(osaka_payload.block().body().transactions().any(|tx| *tx.hash() == blob_tx_hash));
    assert!(matches!(osaka_payload.sidecars(), BlobSidecars::Eip7594(_)));

    node.update_forkchoice(genesis_hash, node.submit_payload(osaka_payload).await?).await?;

    Ok(())
}
