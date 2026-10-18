use super::*;
use crate::{ReceiptSelection, SelectionOperation};
use reth_pureth_receipt::{
    test_utils, MULTIPLE_LOGS_BLOCK_HASH, PROGRESSIVE_RECEIPTS_BLOCK_HASH, SINGLETON_BLOCK_HASH,
};

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
fn deterministic_cases_join_the_exact_stored_eip_snapshot() {
    let provider = DeterministicProvider::new().unwrap();
    for (hash, path, address) in [
        (SINGLETON_BLOCK_HASH, "[0].logs[0].address", 0x11),
        (MULTIPLE_LOGS_BLOCK_HASH, "[0].logs[1].address", 0x33),
        (PROGRESSIVE_RECEIPTS_BLOCK_HASH, "[5].logs[0].address", 0x16),
    ] {
        let snapshot = provider.lookup(hash, ObjectKind::Receipts).unwrap();
        let request = request(hash, path);
        let response = query_snapshot(&request, snapshot, SelectionLimits::default()).unwrap();
        assert_eq!(response.selection.root, snapshot.root());
        assert_eq!(response.selection.root, snapshot.receipt_snapshot().tree().root());
        assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[address; 20]);
        verify_query_response(&request, &response, snapshot.root(), SelectionLimits::default())
            .unwrap();
    }
}

#[test]
fn historical_query_uses_eip_tree_and_preserves_source_context() {
    let (provider, hash) = test_utils::historical_provider(Some(test_utils::singleton_receipts()));
    let service = QueryService::from_blockchain_provider(provider);
    let request = request(hash, "[0].logs[0].address");
    let response = service.query(request.clone()).unwrap();
    assert_eq!(response.block_hash, hash);
    assert_eq!(response.block_status, "canonical");
    assert_eq!(response.root_context, "reth_experimental_unanchored");
    assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[0x11; 20]);
    verify_query_response(&request, &response, response.selection.root, SelectionLimits::default())
        .unwrap();
}

#[test]
fn missing_receipts_and_conversion_failures_keep_typed_causes() {
    let (provider, hash) = test_utils::historical_provider(None);
    assert!(matches!(
        QueryService::from_blockchain_provider(provider).query(request(hash, "[0].status")),
        Err(QueryError::Acquisition(HistoricalAcquisitionError::ReceiptsUnavailable))
    ));
    let mut receipts = test_utils::singleton_receipts();
    receipts[0].tx_type = alloy_consensus::TxType::Eip1559;
    let (provider, hash) = test_utils::historical_provider(Some(receipts));
    assert!(matches!(
        QueryService::from_blockchain_provider(provider).query(request(hash, "[0].status")),
        Err(QueryError::Acquisition(HistoricalAcquisitionError::Snapshot(
            reth_pureth_receipt::Eip6466SnapshotError::Conversion(_)
        )))
    ));
}

#[test]
fn invalid_request_and_cancellation_are_checked_before_lookup() {
    let service = QueryService::new().unwrap();
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
    let service = QueryService::new().unwrap();
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
    let provider = DeterministicProvider::new().unwrap();
    let snapshot = provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Receipts).unwrap();
    assert!(matches!(
        query_snapshot(
            &request(MULTIPLE_LOGS_BLOCK_HASH, "[0].status"),
            snapshot,
            SelectionLimits::default()
        ),
        Err(QueryError::InvalidSnapshot)
    ));
}

#[test]
fn response_verification_rejects_wrong_identity_values_roots_and_witnesses() {
    let service = QueryService::new().unwrap();
    let request = request(SINGLETON_BLOCK_HASH, "[0].logs[0].address");
    let response = service.query(request.clone()).unwrap();
    let root = response.selection.root;
    for mutation in 0..8 {
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
            6 => changed.selection.results[0].witnesses.as_mut().unwrap()[0].node[0] ^= 1,
            _ => changed.selection.results[0]
                .witnesses
                .as_mut()
                .unwrap()
                .push(crate::SelectionWitness { node: B256::ZERO, branch: vec![] }),
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
    let request = request(SINGLETON_BLOCK_HASH, "[0].status");
    let response = QueryService::new().unwrap().query(request.clone()).unwrap();
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
    let service = QueryService::new()
        .unwrap()
        .with_limits(SelectionLimits { request_bytes: 10, ..Default::default() });
    assert!(matches!(
        service.query(request(B256::ZERO, "[0].status")),
        Err(QueryError::Selection(SelectionError::LimitExceeded("request bytes")))
    ));
}
