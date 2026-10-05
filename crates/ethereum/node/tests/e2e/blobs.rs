use alloy_eips::{merge::SLOT_DURATION_SECS, Decodable2718};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    node::Finality, test_chain_spec, test_chain_spec_builder, transaction::TransactionTestContext,
    E2ETestSetupExt,
};
use reth_ethereum_engine_primitives::BlobSidecars;
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_node_ethereum::EthereumNode;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

#[tokio::test]
async fn can_handle_blobs() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec(EthereumHardfork::Cancun);
    let genesis_hash = chain_spec.genesis_hash();
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    // keep genesis finalized, so the blob block can be reorged
    node.set_finality(Finality::Keep);

    // build blob tx
    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(0)).await?;

    // inject blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // mine the blob tx and wait for the pool to remove it
    let blob_payload = node.advance_block_synced().await?;
    assert!(blob_payload.block().body().transactions().any(|tx| *tx.hash() == blob_tx_hash));

    // reorg the blob block out with a block on genesis, which can't include the blob tx anymore
    let block_hash = node.advance_block_on(genesis_hash).await?.block().hash();

    // wait for the pool to process the reorg, and then re-inject the blob tx
    node.wait_for_pool_head(block_hash).await?;
    node.wait_for_pooled([blob_tx_hash]).await?;

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

    // import the blob payload
    node.import_payload(blob_payload).await?;

    Ok(())
}

#[tokio::test]
async fn blob_conversion_at_osaka() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Keep genesis outside the conversion window. Importing the Prague payload below starts
    // conversion two slots before Osaka, after all legacy sidecar assertions have completed.
    let prague_timestamp = 2 * SLOT_DURATION_SECS;
    let osaka_timestamp = prague_timestamp + 2 * SLOT_DURATION_SECS;

    let chain_spec = Arc::new(
        test_chain_spec_builder().prague_activated().with_osaka_at(osaka_timestamp).build(),
    );
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec)
        .with_rpc_modifier(|rpc| rpc.with_force_blob_sidecar_upcasting())
        .build_single()
        .await?;

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
    node.set_next_payload_timestamp(prague_timestamp)?;
    let prague_payload = node.new_payload().await?;
    assert_eq!(prague_payload.block().timestamp, prague_timestamp);
    assert!(prague_payload.block().body().transactions().any(|tx| *tx.hash() == blob_tx_hash));
    assert!(matches!(prague_payload.sidecars(), BlobSidecars::Eip4844(_)));

    // inject second blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(second_blob).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // assert that it still has a legacy sidecar
    assert!(envelope.as_eip4844().unwrap().tx().sidecar().unwrap().is_eip4844());
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // Import the Prague payload to trigger conversion only after checking both legacy sidecars.
    node.import_payload(prague_payload).await?;

    // wait for the pool to convert the sidecar ahead of the Osaka activation
    node.wait_for_pool(|pool| {
        pool.get_blob(blob_tx_hash).ok().flatten().is_some_and(|sidecar| sidecar.is_eip7594())
    })
    .await?;

    // fetch second blob tx from rpc again
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // assert that it was converted to eip7594
    assert!(envelope.as_eip4844().unwrap().tx().sidecar().unwrap().is_eip7594());
    // validate sidecar
    TransactionTestContext::validate_sidecar(envelope);

    // Build first Osaka payload
    node.set_next_payload_timestamp(osaka_timestamp)?;
    let osaka_payload = node.new_payload().await?;
    assert_eq!(osaka_payload.block().timestamp, osaka_timestamp);

    // Assert that it includes the second blob tx with eip7594 sidecar
    assert!(osaka_payload.block().body().transactions().any(|tx| *tx.hash() == blob_tx_hash));
    assert!(matches!(osaka_payload.sidecars(), BlobSidecars::Eip7594(_)));

    node.import_payload(osaka_payload).await?;

    Ok(())
}
