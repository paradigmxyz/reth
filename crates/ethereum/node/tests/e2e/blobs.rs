use alloy_eips::Decodable2718;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    test_chain_spec, test_chain_spec_builder, transaction::TransactionTestContext, E2ETestSetupExt,
};
use reth_ethereum_engine_primitives::BlobSidecars;
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_node_ethereum::EthereumNode;
use reth_transaction_pool::TransactionPool;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::test]
async fn can_handle_blobs() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec(EthereumHardfork::Cancun);
    let genesis_hash = chain_spec.genesis_hash();
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;

    // inject normal tx
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.signer(1)).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    // build payload with normal tx
    let payload = node.new_payload().await?;

    // clean the pool
    node.inner.pool.remove_transactions(vec![tx_hash]);

    // build blob tx
    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(0)).await?;

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

    let chain_spec = test_chain_spec(EthereumHardfork::Osaka);
    let genesis_hash = chain_spec.genesis_hash();
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec)
        .with_rpc_modifier(|rpc| rpc.with_force_blob_sidecar_upcasting())
        .build_single()
        .await?;

    // build blob tx
    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(0)).await?;

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

    let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    // Osaka activates in 2 slots
    let osaka_timestamp = current_timestamp + 24;

    let chain_spec = Arc::new(
        test_chain_spec_builder().prague_activated().with_osaka_at(osaka_timestamp).build(),
    );
    let genesis_hash = chain_spec.genesis_hash();
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec)
        .with_rpc_modifier(|rpc| rpc.with_force_blob_sidecar_upcasting())
        .build_single()
        .await?;

    // build a dummy payload at `current_timestamp`
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.signer(0)).await;
    node.rpc.inject_tx(raw_tx).await?;
    node.payload.timestamp = current_timestamp - 1;
    node.advance_block().await?;

    // build blob txs
    let first_blob = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(1)).await?;
    let second_blob = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(2)).await?;

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
