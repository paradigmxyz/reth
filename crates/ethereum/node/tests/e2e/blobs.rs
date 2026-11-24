use alloy_eips::Decodable2718;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    node::Finality, test_chain_spec, transaction::TransactionTestContext, E2ETestSetupExt,
};
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_node_ethereum::EthereumNode;

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
