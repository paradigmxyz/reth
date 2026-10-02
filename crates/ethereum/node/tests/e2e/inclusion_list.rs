use alloy_eips::{
    eip2718::{Decodable2718, Encodable2718},
    eip7840::BlobParams,
};
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::{BogotaPayloadFields, ExecutionData, ExecutionPayloadSidecar};
use reth_e2e_test_utils::{
    test_chain_spec_builder, transaction::TransactionTestContext, E2ETestSetupExt,
};
use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

/// Builds a block carrying one blob transaction under a blob schedule of `max_blob_count` blobs
/// per block, submits it with an inclusion list holding a second, otherwise valid blob
/// transaction, and returns the engine's `inclusionListSatisfied` verdict.
async fn blob_inclusion_list_verdict(max_blob_count: u64) -> eyre::Result<Option<bool>> {
    let mut chain_spec = test_chain_spec_builder().bogota_activated().build();
    chain_spec.blob_params.scheduled =
        vec![(0, BlobParams { target_blob_count: 1, max_blob_count, ..BlobParams::osaka() })];
    let (mut node, wallet) = EthereumNode::test_setup(1, Arc::new(chain_spec))
        .with_rpc_modifier(|rpc| rpc.with_force_blob_sidecar_upcasting())
        .build_single()
        .await?;

    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(0)).await?;
    node.rpc.inject_tx(blob_tx).await?;
    let payload = node.new_payload().await?;
    let block_hash = payload.block().hash();
    assert_eq!(payload.block().body().transactions.len(), 1);

    // The list carries the consensus encoding, without the sidecar.
    let pooled = TransactionTestContext::tx_with_blobs_bytes(1, wallet.signer(1)).await?;
    let listed = TransactionSigned::from(PooledTransactionVariant::decode_2718_exact(&pooled)?);
    let inclusion_list = vec![Bytes::from(listed.encoded_2718())];

    let mut data = ExecutionData::from(payload);
    let cancun = data.sidecar.cancun().cloned().expect("Bogota payload has Cancun fields");
    let prague = data.sidecar.prague().cloned().expect("Bogota payload has Prague fields");
    data.sidecar = ExecutionPayloadSidecar::v6(
        cancun,
        prague,
        BogotaPayloadFields::new(inclusion_list.clone()),
    );

    let engine = &node.inner.add_ons_handle.beacon_engine_handle;
    let status = engine.new_payload_with_inclusion_list(data, inclusion_list).await?;
    assert!(status.is_valid(), "{status:?}");

    Ok(engine.inclusion_list_status(block_hash).await?)
}

#[tokio::test]
async fn inclusion_list_blob_tx_over_the_block_blob_budget_is_not_appendable() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // The block's single blob already exhausts the budget, so the listed transaction could not
    // have been appended and the list is satisfied.
    assert_eq!(blob_inclusion_list_verdict(1).await?, Some(true));
    Ok(())
}

#[tokio::test]
async fn inclusion_list_blob_tx_within_the_block_blob_budget_is_appendable() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // With room for a second blob, the block left out a transaction it could have included.
    assert_eq!(blob_inclusion_list_verdict(2).await?, Some(false));
    Ok(())
}
