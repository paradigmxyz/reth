use alloy_eips::{
    eip2718::{Decodable2718, Encodable2718},
    eip7840::BlobParams,
};
use alloy_primitives::{Address, Bytes};
use alloy_rpc_types_engine::{BogotaPayloadFields, ExecutionData, ExecutionPayloadSidecar};
use alloy_rpc_types_eth::TransactionRequest;
use reth_e2e_test_utils::{
    test_chain_spec_builder, transaction::TransactionTestContext, E2ETestSetupExt,
};
use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

/// Builds a block carrying one blob transaction under a blob schedule of `max_blob_count` blobs
/// per block, submits it with an inclusion list holding a second, otherwise valid blob
/// transaction, optionally followed by an appendable transfer, and returns the engine verdict.
async fn blob_inclusion_list_verdict(
    max_blob_count: u64,
    append_transfer: bool,
    already_executed: bool,
) -> eyre::Result<Option<bool>> {
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
    let mut inclusion_list = vec![Bytes::from(listed.encoded_2718())];
    if append_transfer {
        let transfer = TransactionRequest {
            chain_id: Some(1),
            nonce: Some(0),
            gas: Some(210_000),
            to: Some(Address::ZERO.into()),
            max_fee_per_gas: Some(20_000_000_000),
            max_priority_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };
        inclusion_list
            .push(TransactionTestContext::sign_tx_bytes(wallet.signer(1), transfer).await);
    }

    let mut data = ExecutionData::from(payload);
    let engine = &node.inner.add_ons_handle.beacon_engine_handle;
    if already_executed {
        // Import without a list first, so the later verdict must use the post-state fallback.
        let status = engine.new_payload(data.clone()).await?;
        assert!(status.is_valid(), "{status:?}");
    }
    let cancun = data.sidecar.cancun().cloned().expect("Bogota payload has Cancun fields");
    let prague = data.sidecar.prague().cloned().expect("Bogota payload has Prague fields");
    data.sidecar = ExecutionPayloadSidecar::v6(
        cancun,
        prague,
        BogotaPayloadFields::new(inclusion_list.clone()),
    );

    let status = engine.new_payload_with_inclusion_list(data, inclusion_list).await?;
    assert!(status.is_valid(), "{status:?}");

    Ok(engine.inclusion_list_status(block_hash).await?)
}

#[tokio::test]
async fn inclusion_list_blob_tx_is_skipped_with_a_full_blob_budget() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    assert_eq!(blob_inclusion_list_verdict(1, false, false).await?, Some(true));
    Ok(())
}

#[tokio::test]
async fn inclusion_list_blob_tx_is_skipped_with_spare_blob_budget() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    for already_executed in [false, true] {
        assert_eq!(blob_inclusion_list_verdict(2, false, already_executed).await?, Some(true));
    }
    Ok(())
}

#[tokio::test]
async fn inclusion_list_checks_entries_after_a_skipped_blob() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    for already_executed in [false, true] {
        assert_eq!(blob_inclusion_list_verdict(2, true, already_executed).await?, Some(false));
    }
    Ok(())
}
