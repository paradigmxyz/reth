use alloy_consensus::BlockHeader;
use alloy_eips::{
    eip2718::{Decodable2718, Encodable2718},
    eip7840::BlobParams,
};
use alloy_primitives::{bytes, Address, Bytes, U256};
use alloy_rpc_types_engine::{BogotaPayloadFields, ExecutionData, ExecutionPayloadSidecar};
use alloy_rpc_types_eth::TransactionRequest;
use reth_chainspec::ChainSpecBuilder;
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

async fn inclusion_list_gas_verdict(already_executed: bool) -> eyre::Result<()> {
    let chain_spec = Arc::new(test_chain_spec_builder().bogota_activated().build());
    let mut genesis = chain_spec.genesis.clone();
    let burner = Address::repeat_byte(0x42);
    genesis.alloc.entry(burner).or_default().code = Some(bytes!("5b600056"));
    let chain_spec = ChainSpecBuilder::from(&chain_spec).genesis(genesis).build();
    let (mut node, wallet) =
        EthereumNode::test_setup(1, Arc::new(chain_spec)).build_single().await?;
    for (nonce, to, gas, value) in [(0, burner, 1_000_000, 0u64), (1, Address::ZERO, 210_000, 1)] {
        let transaction = TransactionRequest {
            chain_id: Some(1),
            nonce: Some(nonce),
            gas: Some(gas),
            to: Some(to.into()),
            value: Some(U256::from(value)),
            max_fee_per_gas: Some(20_000_000_000),
            max_priority_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };
        let encoded = TransactionTestContext::sign_tx_bytes(wallet.signer(0), transaction).await;
        node.rpc.inject_tx(encoded).await?;
    }
    let payload = node.new_payload().await?;
    assert_eq!(payload.block().body().transactions.len(), 2);
    let block_hash = payload.block().hash();
    // Execution gas dominates this block. The full pending limit exceeds execution room by
    // one, while its capped execution grant and full state-gas reservation both fit.
    let pending = TransactionRequest {
        chain_id: Some(1),
        nonce: Some(0),
        gas: Some(payload.block().gas_limit() - payload.block().gas_used() + 1),
        to: Some(Address::ZERO.into()),
        max_fee_per_gas: Some(20_000_000_000),
        max_priority_fee_per_gas: Some(1_000_000_000),
        ..Default::default()
    };
    let list = vec![TransactionTestContext::sign_tx_bytes(wallet.signer(1), pending).await];
    let mut data = ExecutionData::from(payload);
    let engine = &node.inner.add_ons_handle.beacon_engine_handle;
    if already_executed {
        assert!(engine.new_payload(data.clone()).await?.is_valid());
    }
    data.sidecar = ExecutionPayloadSidecar::v6(
        data.sidecar.cancun().cloned().unwrap(),
        data.sidecar.prague().cloned().unwrap(),
        BogotaPayloadFields::new(list.clone()),
    );
    for _ in 0..3 {
        let status = engine.new_payload_with_inclusion_list(data.clone(), list.clone()).await?;
        assert!(status.is_valid(), "{status:?}");
        assert_eq!(engine.inclusion_list_status(block_hash).await?, Some(false));
    }
    Ok(())
}

#[tokio::test]
async fn repeated_payload_preserves_separate_inclusion_list_gas_budgets() -> eyre::Result<()> {
    for already_executed in [false, true] {
        inclusion_list_gas_verdict(already_executed).await?;
    }
    Ok(())
}
