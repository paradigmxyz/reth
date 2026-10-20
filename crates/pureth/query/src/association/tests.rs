use super::*;
use alloy_primitives::Bytes;
use reth_pureth_block::{ExecutionRequestsSnapshot, PayloadLimits};
use reth_pureth_receipt::eip6466::{Eip6466ReceiptSnapshot, Receipts};
use std::collections::BTreeSet;

fn fixtures() -> Vec<(ExecutionPayloadSnapshot, B256)> {
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("../../../block/tests/fixtures/payload.json")).unwrap();
    reference["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let bytes = |field: &str| case[field].as_str().unwrap().parse::<Bytes>().unwrap();
            let receipts = Eip6466ReceiptSnapshot::build(
                Receipts::from_ssz_bytes(&bytes("receipts")).unwrap(),
            )
            .unwrap();
            let requests = ExecutionRequestsSnapshot::from_ssz(&bytes("requests")).unwrap();
            let payload = ExecutionPayloadSnapshot::from_ssz(
                &bytes("payload"),
                receipts,
                requests,
                PayloadLimits::default(),
            )
            .unwrap();
            (payload, case["root"].as_str().unwrap().parse().unwrap())
        })
        .collect()
}

fn request(index: u64) -> TransactionReceiptRequest {
    TransactionReceiptRequest { block_hash: B256::repeat_byte(0x41), transaction_index: index }
}

fn response(payload: &ExecutionPayloadSnapshot, index: u64) -> TransactionReceiptResponse {
    select_payload(
        payload,
        &request(index),
        "unknown",
        SelectionLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[test]
fn associations_match_independent_payload_roots_for_every_transaction_layout() {
    let mut selectors = BTreeSet::new();
    for (payload, root) in fixtures() {
        assert_eq!(payload.root(), root);
        for (index, transaction) in
            payload.components().transactions.transactions().iter().enumerate()
        {
            selectors.insert(transaction.selector());
            let response = response(&payload, index as u64);
            verify_transaction_receipt(
                &request(index as u64),
                &response,
                root,
                SelectionLimits::default(),
            )
            .unwrap();
            assert_eq!(response.transaction_hash, transaction.original_transaction_hash().unwrap());
            assert!(verify_transaction_receipt(
                &request(index as u64),
                &response,
                B256::ZERO,
                SelectionLimits::default()
            )
            .is_err());
            let json = serde_json::to_string(&response).unwrap();
            assert!(!json.contains("gindex"));
            assert_eq!(
                serde_json::from_str::<TransactionReceiptResponse>(&json).unwrap(),
                response
            );
        }
        let end = payload.components().transactions.transactions().len() as u64;
        assert!(select_payload(
            &payload,
            &request(end),
            "unknown",
            SelectionLimits::default(),
            &AtomicBool::new(false)
        )
        .is_err());
    }
    assert_eq!(selectors, (1..=10).collect());
}

#[test]
fn associations_reject_changed_values_missing_links_and_unused_context() {
    let fixtures = fixtures();
    let (payload, root) = &fixtures[2];
    let response = response(payload, 0);
    for mutation in 0..17 {
        let mut changed = response.clone();
        match mutation {
            0 => changed.block_hash = B256::ZERO,
            1 => changed.transaction_index = 1,
            2 => changed.root_context = "anchored".into(),
            3 => changed.block_status = "invalid".into(),
            4 => changed.transaction_hash = B256::ZERO,
            5 => changed.summary.root = B256::ZERO,
            6 => changed.transaction.root = B256::ZERO,
            7 => changed.receipt.root = B256::ZERO,
            8 => changed.summary.results.swap(0, 1),
            9 => changed.summary.multiproof = None,
            10 => changed.transaction.results[0].values_ssz[0] = Bytes::from_static(&[0]),
            11 => changed.receipt.results[0].values_ssz[0] = Bytes::from_static(&[1]),
            12 => changed.summary.multiproof.as_mut().unwrap().helpers.push(B256::ZERO),
            13 => changed.transaction.multiproof.as_mut().unwrap().targets.push(B256::ZERO),
            14 => changed.receipt.results[0].matches = Some(vec![]),
            15 => changed.receipt.results[0].values_ssz.push(Bytes::new()),
            _ => changed.receipt.multiproof.as_mut().unwrap().helpers[0][0] ^= 1,
        }
        assert!(
            verify_transaction_receipt(&request(0), &changed, *root, SelectionLimits::default())
                .is_err(),
            "mutation {mutation}"
        );
    }
    let mut value = serde_json::to_value(&response).unwrap();
    value["unused_link"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TransactionReceiptResponse>(value).is_err());
}

#[test]
fn associations_reject_cross_index_and_cross_payload_proofs() {
    let fixtures = fixtures();
    let (payload, root) = &fixtures[2];
    let original = response(payload, 0);
    let other_index = response(payload, 1);
    let other_payload = response(&fixtures[1].0, 0);
    for changed in [
        TransactionReceiptResponse { transaction: other_index.transaction, ..original.clone() },
        TransactionReceiptResponse { receipt: other_index.receipt, ..original.clone() },
        TransactionReceiptResponse { summary: other_payload.summary, ..original.clone() },
        TransactionReceiptResponse { transaction: other_payload.transaction, ..original.clone() },
        TransactionReceiptResponse { receipt: other_payload.receipt, ..original },
    ] {
        assert!(verify_transaction_receipt(
            &request(0),
            &changed,
            *root,
            SelectionLimits::default()
        )
        .is_err());
    }
}

#[test]
fn association_limits_apply_to_the_complete_response_and_witness_set() {
    let fixtures = fixtures();
    let (payload, root) = &fixtures[2];
    let response = response(payload, 0);
    let hashes = |response: &SelectionResponse| {
        let proof = response.multiproof.as_ref().unwrap();
        proof.targets.len() + proof.helpers.len()
    };
    let maximum = [&response.summary, &response.transaction, &response.receipt]
        .into_iter()
        .map(hashes)
        .max()
        .unwrap();
    let total =
        hashes(&response.summary) + hashes(&response.transaction) + hashes(&response.receipt);
    assert!(total > maximum);
    let limits = SelectionLimits { witness_hashes: total, ..Default::default() };
    verify_transaction_receipt(&request(0), &response, *root, limits).unwrap();
    select_payload(payload, &request(0), "unknown", limits, &AtomicBool::new(false)).unwrap();
    let serialized = serde_json::to_vec(&response).unwrap();
    for limits in [
        SelectionLimits { selections: 3, ..Default::default() },
        SelectionLimits { targets: 3, ..Default::default() },
        SelectionLimits { witness_hashes: maximum, ..Default::default() },
        SelectionLimits { source_bytes: 1, ..Default::default() },
        SelectionLimits { request_bytes: 1, ..Default::default() },
        SelectionLimits { response_bytes: serialized.len() - 1, ..Default::default() },
    ] {
        assert!(matches!(
            verify_transaction_receipt(&request(0), &response, *root, limits),
            Err(SelectionError::LimitExceeded(_))
        ));
        assert!(matches!(
            select_payload(payload, &request(0), "unknown", limits, &AtomicBool::new(false)),
            Err(SelectionError::LimitExceeded(_))
        ));
    }
    for produce in [false, true] {
        let accepts = |source_bytes| {
            let limits = SelectionLimits { source_bytes, ..Default::default() };
            if produce {
                select_payload(payload, &request(0), "unknown", limits, &AtomicBool::new(false))
                    .is_ok()
            } else {
                verify_transaction_receipt(&request(0), &response, *root, limits).is_ok()
            }
        };
        let mut low = 0;
        let mut high = SelectionLimits::default().source_bytes;
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if accepts(middle) {
                high = middle;
            } else {
                low = middle;
            }
        }
        assert!(accepts(high));
        assert!(!accepts(high - 1));
    }
    let cancelled = AtomicBool::new(false);
    let limits = SelectionLimits::default();
    let summary = ComponentSnapshot::ExecutionSummary(payload.summary());
    let transactions = ComponentSnapshot::Transactions(&payload.components().transactions);
    let (_, summary_work) =
        select_component_with_usage(summary, &summary_request(), limits, &cancelled).unwrap();
    let (_, transaction_work) =
        select_component_with_usage(transactions, &item_request(0), limits, &cancelled).unwrap();
    let (_, receipt_work) =
        select_receipt_with_usage(&payload.components().receipts, 0, limits, &cancelled).unwrap();
    let maximum = summary_work.max(transaction_work).max(receipt_work);
    let limits = SelectionLimits { source_bytes: maximum, ..limits };
    select_component_with_usage(summary, &summary_request(), limits, &cancelled).unwrap();
    select_component_with_usage(transactions, &item_request(0), limits, &cancelled).unwrap();
    select_receipt_with_usage(&payload.components().receipts, 0, limits, &cancelled).unwrap();
    assert!(matches!(
        select_payload(payload, &request(0), "unknown", limits, &cancelled),
        Err(SelectionError::LimitExceeded(_))
    ));
}

#[test]
fn sparse_associations_do_not_use_filter_scan_limits() {
    let fixtures = fixtures();
    let (payload, root) = &fixtures[2];
    let expected = response(payload, 1);
    let end = payload.components().transactions.transactions().len() as u64;
    for receipts in [0, 1] {
        let limits = SelectionLimits { receipts, ..Default::default() };
        verify_transaction_receipt(&request(1), &expected, *root, limits).unwrap();
        assert_eq!(
            select_payload(payload, &request(1), "unknown", limits, &AtomicBool::new(false))
                .unwrap(),
            expected
        );
        assert_eq!(
            select_payload(payload, &request(end), "unknown", limits, &AtomicBool::new(false)),
            Err(SelectionError::OutOfBounds)
        );
    }
}

#[test]
fn association_cancellation_is_checked_before_any_proof_work() {
    let fixtures = fixtures();
    let (payload, root) = &fixtures[1];
    let response = response(payload, 0);
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        select_payload(payload, &request(0), "unknown", SelectionLimits::default(), &cancelled),
        Err(SelectionError::Cancelled)
    );
    assert_eq!(
        verify_transaction_receipt_with_cancel(
            &request(0),
            &response,
            *root,
            SelectionLimits::default(),
            &cancelled
        ),
        Err(SelectionError::Cancelled)
    );
}
