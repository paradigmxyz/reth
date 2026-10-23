use super::*;
use crate::{test_utils, FilterOutput, FilterPredicate, ReceiptSelection, SelectionOperation};
use reth_pureth_receipt::test_utils as receipt_test_utils;

fn request(block_hash: B256, path: &str) -> QueryRequest {
    QueryRequest {
        block_hash,
        object: "receipts".to_owned(),
        selection: SelectionRequest {
            selections: vec![ReceiptSelection {
                path: path.to_owned(),
                operation: SelectionOperation::Value {},
            }],
            include_proof: true,
        },
    }
}

#[test]
fn provider_cases_join_the_acquired_eip_snapshot() {
    for (count, path, address) in [
        (1, "[0].logs[0].address", 0x11),
        (2, "[0].logs[1].address", 0x33),
        (6, "[5].logs[0].address", 0x11),
    ] {
        let snapshot = test_utils::snapshot(count);
        let request = request(snapshot.block_hash(), path);
        let response = query_snapshot(&request, &snapshot, SelectionLimits::default()).unwrap();
        assert_eq!(response.selection.root, snapshot.root());
        assert_eq!(response.selection.root, snapshot.receipt_snapshot().tree().root());
        assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[address; 20]);
        verify_query_response(&request, &response, snapshot.root(), SelectionLimits::default())
            .unwrap();
    }
}

#[test]
fn filters_use_the_existing_service_and_validate_predicates_before_lookup() {
    let snapshot = test_utils::snapshot(6);
    let mut query = request(snapshot.block_hash(), ".");
    query.selection.selections[0].operation =
        SelectionOperation::Filter { output: FilterOutput::Receipts, predicates: Vec::new() };
    let response = query_snapshot(&query, &snapshot, SelectionLimits::default()).unwrap();
    assert_eq!(response.selection.results[0].values_ssz.len(), 6);
    verify_query_response(&query, &response, snapshot.root(), SelectionLimits::default()).unwrap();
    query.block_hash = B256::ZERO;
    query.selection.selections[0].operation = SelectionOperation::Filter {
        output: FilterOutput::Logs,
        predicates: vec![FilterPredicate::GasUsed { min: 2, max: 1 }],
    };
    assert!(matches!(
        test_utils::service().0.query(query),
        Err(QueryError::Selection(SelectionError::OutOfBounds))
    ));
}

#[test]
fn historical_query_uses_eip_tree_and_preserves_source_context() {
    let (provider, hash) =
        receipt_test_utils::historical_provider(Some(receipt_test_utils::singleton_receipts()));
    let expected_root =
        RethRootProvider::new(provider.clone()).lookup(hash, ObjectKind::Receipts).unwrap().root();
    let service = QueryService::from_blockchain_provider(provider);
    let request = request(hash, "[0].logs[0].address");
    let response = service.query(request.clone()).unwrap();
    assert_eq!(response.block_hash, hash);
    assert_eq!(response.block_status, "canonical");
    assert_eq!(response.root_context, "reth_experimental_unanchored");
    assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[0x11; 20]);
    verify_query_response(&request, &response, expected_root, SelectionLimits::default()).unwrap();
}

#[test]
fn self_consistent_proofs_from_another_block_do_not_replace_the_expected_root() {
    let expected = test_utils::snapshot(1);
    let expected_root = expected.root();
    let requested = request(expected.block_hash(), "[0].status");
    let (provider, hash) = test_utils::provider(2);
    let alternate = request(hash, "[0].status");
    let mut response = QueryService::from_blockchain_provider(provider).query(alternate).unwrap();
    response.block_hash = requested.block_hash;
    assert_ne!(response.selection.root, expected_root);
    assert!(verify_query_response(
        &requested,
        &response,
        response.selection.root,
        SelectionLimits::default(),
    )
    .is_ok());
    assert_eq!(
        verify_query_response(&requested, &response, expected_root, SelectionLimits::default()),
        Err(ResponseVerificationError::Selection(SelectionError::InvalidContext)),
    );
}

#[test]
fn missing_receipts_and_conversion_failures_keep_typed_causes() {
    let (provider, hash) = receipt_test_utils::historical_provider(None);
    assert!(matches!(
        QueryService::from_blockchain_provider(provider).query(request(hash, "[0].status")),
        Err(QueryError::Acquisition(HistoricalAcquisitionError::ReceiptsUnavailable))
    ));
    let mut receipts = receipt_test_utils::singleton_receipts();
    receipts[0].tx_type = alloy_consensus::TxType::Eip1559;
    let (provider, hash) = receipt_test_utils::historical_provider(Some(receipts));
    assert!(matches!(
        QueryService::from_blockchain_provider(provider).query(request(hash, "[0].status")),
        Err(QueryError::Acquisition(HistoricalAcquisitionError::Snapshot(
            reth_pureth_receipt::Eip6466SnapshotError::Conversion(_)
        )))
    ));
}

#[test]
fn invalid_request_and_cancellation_are_checked_before_lookup() {
    let (service, _) = test_utils::service();
    let mut invalid = request(B256::ZERO, "[0].status");
    invalid.object = "withdrawals".to_owned();
    assert!(matches!(service.query(invalid), Err(QueryError::UnsupportedObject)));
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        service.query_with_cancel(request(B256::ZERO, "[0].status"), &cancelled),
        Err(QueryError::Selection(SelectionError::Cancelled))
    ));
    let mut invalid = request(B256::ZERO, "[0].status");
    invalid.selection.selections.clear();
    assert!(matches!(
        service.query(invalid),
        Err(QueryError::Selection(SelectionError::LimitExceeded("selections")))
    ));
}

#[test]
fn malformed_paths_and_range_expansion_fail_before_lookup() {
    let (service, _) = test_utils::service();
    for path in ["", "not_a_path", "[01].status", "[18446744073709551616].status"] {
        assert!(matches!(
            service.query(request(B256::ZERO, path)),
            Err(QueryError::Selection(SelectionError::InvalidPath))
        ));
    }
    for (operation, expected) in [
        (SelectionOperation::Range { start: 2, end: 1 }, SelectionError::OutOfBounds),
        (
            SelectionOperation::Slice { start: 0, end: 1025 },
            SelectionError::LimitExceeded("targets"),
        ),
        (
            SelectionOperation::Range { start: 0, end: u64::MAX },
            SelectionError::LimitExceeded("targets"),
        ),
    ] {
        let mut invalid = request(B256::ZERO, "[0].logs[0].data");
        invalid.selection.selections[0].operation = operation;
        assert!(
            matches!(service.query(invalid), Err(QueryError::Selection(error)) if error == expected)
        );
    }
    let mut invalid = request(B256::ZERO, "[0].logs[0].data");
    invalid.selection.selections[0].operation = SelectionOperation::Slice { start: 0, end: 600 };
    invalid.selection.selections.push(ReceiptSelection {
        path: "[1].logs[0].data".to_owned(),
        operation: SelectionOperation::Slice { start: 0, end: 600 },
    });
    assert!(matches!(
        service.query(invalid),
        Err(QueryError::Selection(SelectionError::LimitExceeded("targets")))
    ));
}

#[test]
fn mismatched_snapshot_identity_is_not_substituted() {
    let snapshot = test_utils::snapshot(1);
    assert!(matches!(
        query_snapshot(&request(B256::ZERO, "[0].status"), &snapshot, SelectionLimits::default()),
        Err(QueryError::InvalidSnapshot)
    ));
}

#[test]
fn response_verification_rejects_wrong_identity_values_roots_and_witnesses() {
    let (service, hash) = test_utils::service();
    let request = request(hash, "[0].logs[0].address");
    let response = service.query(request.clone()).unwrap();
    let root = response.selection.root;
    for length in [19, 21] {
        let mut changed = response.clone();
        changed.selection.results[0].values_ssz[0] =
            alloy_primitives::Bytes::from(vec![0x11; length]);
        assert!(
            verify_query_response(&request, &changed, root, SelectionLimits::default()).is_err()
        );
    }
    for mutation in 0..9 {
        let mut changed = response.clone();
        match mutation {
            0 => changed.block_hash = B256::ZERO,
            1 => changed.object = "withdrawals".to_owned(),
            2 => changed.root_context = "anchored".to_owned(),
            3 => changed.block_status = "invalid".to_owned(),
            4 => changed.selection.root = B256::ZERO,
            5 => {
                changed.selection.results[0].values_ssz[0] =
                    alloy_primitives::Bytes::from(vec![0; 20])
            }
            6 => changed.selection.multiproof.as_mut().unwrap().targets[0][0] ^= 1,
            7 => changed.selection.multiproof.as_mut().unwrap().helpers.push(B256::ZERO),
            _ => changed.root_context = "deterministic_test_data".to_owned(),
        }
        assert!(
            verify_query_response(&request, &changed, root, SelectionLimits::default()).is_err()
        );
    }
    assert!(
        verify_query_response(&request, &response, B256::ZERO, SelectionLimits::default()).is_err()
    );
}

#[test]
fn strict_wire_decode_rejects_old_request_and_response_fields() {
    let (service, hash) = test_utils::service();
    let request = request(hash, "[0].status");
    let response = service.query(request.clone()).unwrap();
    for field in ["schema_id", "producer_revision", "path", "gindex"] {
        let mut value = serde_json::to_value(&request).unwrap();
        value[field] = serde_json::json!("obsolete");
        assert!(serde_json::from_value::<QueryRequest>(value).is_err());
        let mut value = serde_json::to_value(&response).unwrap();
        value[field] = serde_json::json!("obsolete");
        assert!(serde_json::from_value::<QueryResponse>(value).is_err());
    }
}

#[test]
fn service_limits_cover_the_full_envelope() {
    let service = test_utils::service()
        .0
        .with_limits(SelectionLimits { request_bytes: 10, ..Default::default() });
    assert!(matches!(
        service.query(request(B256::ZERO, "[0].status")),
        Err(QueryError::Selection(SelectionError::LimitExceeded("request bytes")))
    ));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn payload_provider_preserves_receipts_without_auxiliary_block_sources() {
    let (provider, hash) = test_utils::provider(1);
    let root =
        RethRootProvider::new(provider.clone()).lookup(hash, ObjectKind::Receipts).unwrap().root();
    let service = QueryService::from_payload_provider(
        provider,
        reth_provider::NoopBalStore,
        reth_pureth_block::PayloadLimits::default(),
    );
    let request = request(hash, "[0].status");
    let response = service.query(request.clone()).unwrap();
    verify_query_response(&request, &response, root, SelectionLimits::default()).unwrap();
    let mut component = request;
    component.object = "transactions".into();
    component.selection.selections[0].path = "[0]".into();
    assert!(matches!(service.query(component), Err(QueryError::PayloadAcquisition(_))));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn payload_provider_checks_requests_and_cancellation_before_acquisition() {
    let (provider, _) = test_utils::provider(1);
    let service = QueryService::from_payload_provider(
        provider,
        reth_provider::NoopBalStore,
        reth_pureth_block::PayloadLimits::default(),
    );
    let mut query = request(B256::ZERO, "not_a_path");
    query.object = "transactions".into();
    assert!(matches!(
        service.query(query.clone()),
        Err(QueryError::Selection(SelectionError::InvalidPath))
    ));
    query.selection.selections[0].path = "[0]".into();
    assert!(matches!(
        service.query_with_cancel(query.clone(), &AtomicBool::new(true)),
        Err(QueryError::Selection(SelectionError::Cancelled))
    ));
    query.selection.selections[0].path = ".".into();
    query.selection.selections[0].operation =
        SelectionOperation::Filter { output: FilterOutput::Receipts, predicates: Vec::new() };
    assert!(matches!(
        service.query(query.clone()),
        Err(QueryError::Selection(SelectionError::UnsupportedOperation))
    ));
    query.object = "unknown".into();
    assert!(matches!(service.query(query), Err(QueryError::UnsupportedObject)));
    let association = TransactionReceiptRequest { block_hash: B256::ZERO, transaction_index: 0 };
    assert!(matches!(
        service.transaction_receipt_with_cancel(association.clone(), &AtomicBool::new(true)),
        Err(QueryError::Selection(SelectionError::Cancelled))
    ));
    let bounded = service.with_limits(SelectionLimits { selections: 3, ..Default::default() });
    assert!(matches!(
        bounded.transaction_receipt(association),
        Err(QueryError::Selection(SelectionError::LimitExceeded("selections")))
    ));
}
