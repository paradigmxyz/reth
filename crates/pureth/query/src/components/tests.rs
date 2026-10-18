use super::*;
use reth_pureth_transaction::{TransactionPayload, TransactionSsz};

fn transaction_reference() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tests/fixtures/transactions.json")).unwrap()
}

#[test]
fn all_transaction_variants_fields_presence_and_whole_values_match_independent_reference() {
    let reference = transaction_reference();
    let transactions = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    assert_eq!(transactions.root(), root(&reference["root"]));
    for case in reference["cases"].as_array().unwrap() {
        let operation = match case["operation"].as_str().unwrap() {
            "variant" => SelectionOperation::Variant {},
            "presence" => SelectionOperation::Presence {},
            "whole" => SelectionOperation::Whole {},
            _ => SelectionOperation::Value {},
        };
        let response = checked(
            ComponentSnapshot::Transactions(&transactions),
            &request(&[(case["path"].as_str().unwrap(), operation)]),
        );
        assert_eq!(
            response.selection.results[0].values_ssz,
            vec![bytes(&case["value"])],
            "{}",
            case["path"]
        );
        if let Some(index) = case["gindex"].as_str() {
            let index = u128::from_str_radix(index.trim_start_matches("0x"), 16).unwrap();
            assert_eq!(node_at(transactions.tree(), index).unwrap(), root(&case["node"]));
        }
    }
    checked(
        ComponentSnapshot::Transactions(&transactions),
        &request(&[
            (".", SelectionOperation::Whole {}),
            (".", SelectionOperation::Length {}),
            (".", SelectionOperation::Range { start: 1, end: 4 }),
        ]),
    );
}

#[test]
fn nested_transaction_collections_and_sparse_byte_slices_share_verifiable_evidence() {
    let reference = transaction_reference();
    let transactions = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    for (index, transaction) in transactions.transactions().iter().enumerate() {
        let paths = [
            (format!("[{index}].payload.input_"), SelectionOperation::Length {}),
            (format!("[{index}].payload.input_"), SelectionOperation::Slice { start: 0, end: 0 }),
            (format!("[{index}].signature"), SelectionOperation::Slice { start: 30, end: 36 }),
            (format!("[{index}].signature[65]"), SelectionOperation::Value {}),
        ];
        let batch_request = request(
            &paths
                .iter()
                .map(|(path, operation)| (path.as_str(), operation.clone()))
                .collect::<Vec<_>>(),
        );
        checked(ComponentSnapshot::Transactions(&transactions), &batch_request);
        if transaction.selector() >= 5 {
            checked(
                ComponentSnapshot::Transactions(&transactions),
                &request_for_list(&format!("[{index}].payload.access_list")),
            );
        }
        if transaction.selector() == 9 {
            checked(
                ComponentSnapshot::Transactions(&transactions),
                &request_for_list(&format!("[{index}].payload.blob_versioned_hashes")),
            );
        }
        if transaction.selector() == 10 {
            checked(
                ComponentSnapshot::Transactions(&transactions),
                &request_for_list(&format!("[{index}].payload.authorization_list")),
            );
            checked(
                ComponentSnapshot::Transactions(&transactions),
                &request(&[
                    (
                        &format!("[{index}].payload.authorization_list[0].payload"),
                        SelectionOperation::Variant {},
                    ),
                    (
                        &format!("[{index}].payload.authorization_list[0].payload.address"),
                        SelectionOperation::Value {},
                    ),
                    (
                        &format!("[{index}].payload.authorization_list[0].signature"),
                        SelectionOperation::Whole {},
                    ),
                ]),
            );
        }
    }
}

fn request_for_list(path: &str) -> SelectionRequest {
    request(&[
        (path, SelectionOperation::Whole {}),
        (path, SelectionOperation::Length {}),
        (path, SelectionOperation::Range { start: 0, end: 0 }),
    ])
}

#[test]
fn nested_transaction_fields_and_multichunk_calldata_bind_exact_values() {
    let reference = transaction_reference();
    let originals = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    let source = originals.transactions().iter().find(|tx| tx.selector() == 5).unwrap();
    let access_signature = *source.signature();
    let TransactionPayload::AccessList(mut payload) = source.payload().clone() else {
        unreachable!()
    };
    let input = Bytes::from((0..129).map(|value| value as u8).collect::<Vec<_>>());
    let address = alloy_primitives::Address::repeat_byte(0x42);
    let keys = vec![B256::repeat_byte(0x11), B256::repeat_byte(0x22), B256::repeat_byte(0x11)];
    payload.input = input.clone();
    let mut item = payload.access_list.0[0].clone();
    item.address = address;
    item.storage_keys = keys.clone();
    payload.access_list.0 = vec![item];
    let source = originals.transactions().iter().find(|tx| tx.selector() == 10).unwrap();
    let TransactionPayload::SetCode(mut setcode) = source.payload().clone() else { unreachable!() };
    let source_authorization = &setcode.authorization_list[0];
    let mut replayable = source_authorization.inner().clone();
    replayable.chain_id = alloy_primitives::U256::ZERO;
    let authorization = reth_pureth_transaction::TransactionAuthorization::new_unchecked(
        replayable.clone(),
        source_authorization.y_parity(),
        source_authorization.r(),
        source_authorization.s(),
    );
    replayable.chain_id = alloy_primitives::U256::from(7);
    let explicit = reth_pureth_transaction::TransactionAuthorization::new_unchecked(
        replayable,
        source_authorization.y_parity(),
        source_authorization.r(),
        source_authorization.s(),
    );
    setcode.authorization_list = vec![authorization.clone(), explicit, authorization.clone()];
    let transactions = TransactionSnapshot::build(vec![
        TransactionSsz::new(TransactionPayload::AccessList(payload), access_signature).unwrap(),
        TransactionSsz::new(TransactionPayload::SetCode(setcode), *source.signature()).unwrap(),
    ])
    .unwrap();
    let request = request(&[
        ("[0].payload.input_", SelectionOperation::Slice { start: 31, end: 129 }),
        ("[0].payload.input_", SelectionOperation::Range { start: 32, end: 65 }),
        ("[0].payload.input_[128]", SelectionOperation::Value {}),
        ("[0].payload.input_", SelectionOperation::Slice { start: 129, end: 129 }),
        ("[0].payload.access_list[0].address", SelectionOperation::Value {}),
        ("[0].payload.access_list[0].storage_keys", SelectionOperation::Range { start: 0, end: 3 }),
        ("[0].payload.access_list[0].storage_keys[1]", SelectionOperation::Value {}),
        ("[1].payload.authorization_list[0].payload.chain_id", SelectionOperation::Presence {}),
        ("[1].payload.authorization_list[1].payload.chain_id", SelectionOperation::Value {}),
        ("[1].payload.authorization_list[2].payload.address", SelectionOperation::Value {}),
        ("[1].payload.authorization_list[0].signature", SelectionOperation::Value {}),
    ]);
    let response = checked(ComponentSnapshot::Transactions(&transactions), &request);
    let results = &response.selection.results;
    assert_eq!(results[0].values_ssz, vec![Bytes::copy_from_slice(&input[31..129])]);
    assert_eq!(
        results[1].values_ssz,
        input[32..65].iter().map(|value| Bytes::from(vec![*value])).collect::<Vec<_>>()
    );
    assert_eq!(results[2].values_ssz, vec![Bytes::from_static(&[128])]);
    assert_eq!(results[3].values_ssz, vec![Bytes::new()]);
    assert_eq!(results[4].values_ssz, vec![Bytes::copy_from_slice(address.as_slice())]);
    assert_eq!(
        results[5].values_ssz,
        keys.iter().map(|key| Bytes::copy_from_slice(key.as_slice())).collect::<Vec<_>>()
    );
    assert_eq!(results[6].values_ssz, vec![Bytes::copy_from_slice(keys[1].as_slice())]);
    assert_eq!(results[7].values_ssz, vec![Bytes::from_static(&[0])]);
    assert_eq!(
        results[8].values_ssz,
        vec![Bytes::copy_from_slice(&alloy_primitives::U256::from(7).to_le_bytes::<32>())]
    );
    assert_eq!(
        results[9].values_ssz,
        vec![Bytes::copy_from_slice(authorization.inner().address.as_slice())]
    );
    let mut signature = vec![0];
    signature.extend_from_slice(&authorization.r().to_be_bytes::<32>());
    signature.extend_from_slice(&authorization.s().to_be_bytes::<32>());
    signature.push(authorization.y_parity());
    assert_eq!(results[10].values_ssz, vec![Bytes::from(signature)]);
    for index in [0, 5, 8, 10] {
        let mut changed = response.clone();
        let mut value = changed.selection.results[index].values_ssz[0].to_vec();
        value[0] ^= 1;
        changed.selection.results[index].values_ssz[0] = Bytes::from(value);
        assert!(verify_component_selection(
            ComponentObject::Transactions,
            &request,
            &changed,
            transactions.root(),
            SelectionLimits::default()
        )
        .is_err());
    }
}

#[test]
fn transaction_proofs_reject_changed_values_helpers_variants_and_unsupported_operations() {
    let reference = transaction_reference();
    let transactions = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    let request = request(&[
        ("[0].payload", SelectionOperation::Variant {}),
        ("[0].payload.nonce", SelectionOperation::Value {}),
        ("[0].signature", SelectionOperation::Slice { start: 0, end: 5 }),
    ]);
    let response = checked(ComponentSnapshot::Transactions(&transactions), &request);
    for changed in 0..3 {
        let mut corrupt = response.clone();
        let mut value = corrupt.selection.results[changed].values_ssz[0].to_vec();
        value[0] ^= 1;
        corrupt.selection.results[changed].values_ssz[0] = Bytes::from(value);
        assert!(verify_component_selection(
            ComponentObject::Transactions,
            &request,
            &corrupt,
            transactions.root(),
            SelectionLimits::default()
        )
        .is_err());
    }
    let mut corrupt = response;
    corrupt.selection.multiproof.as_mut().unwrap().helpers.push(B256::ZERO);
    assert!(verify_component_selection(
        ComponentObject::Transactions,
        &request,
        &corrupt,
        transactions.root(),
        SelectionLimits::default()
    )
    .is_err());
    for (path, operation) in [
        ("[0].payload.chain_id", SelectionOperation::Value {}),
        ("[0].payload.nonce", SelectionOperation::Slice { start: 0, end: 1 }),
        ("[0].signature", SelectionOperation::Variant {}),
        ("[0].payload.nonexistent", SelectionOperation::Presence {}),
        ("[0].signature[66]", SelectionOperation::Value {}),
    ] {
        assert!(select_component(
            ComponentSnapshot::Transactions(&transactions),
            &self::request(&[(path, operation)]),
            SelectionLimits::default()
        )
        .is_err());
    }
    assert_eq!(
        select_component_with_cancel(
            ComponentSnapshot::Transactions(&transactions),
            &request,
            SelectionLimits::default(),
            &AtomicBool::new(true)
        ),
        Err(SelectionError::Cancelled)
    );
}

#[test]
fn absent_transaction_fields_are_authenticated_without_becoming_zero_values() {
    let reference = transaction_reference();
    let transactions = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    for (index, transaction) in transactions.transactions().iter().enumerate() {
        for (field, present) in [
            ("access_list", transaction.selector() >= 5),
            ("max_priority_fees_per_gas", transaction.selector() >= 7),
            ("blob_versioned_hashes", transaction.selector() == 9),
            ("authorization_list", transaction.selector() == 10),
        ] {
            let path = format!("[{index}].payload.{field}");
            let response = checked(
                ComponentSnapshot::Transactions(&transactions),
                &request(&[(&path, SelectionOperation::Presence {})]),
            );
            assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[u8::from(present)]);
            if !present {
                assert_eq!(
                    select_component(
                        ComponentSnapshot::Transactions(&transactions),
                        &request(&[(&path, SelectionOperation::Value {})]),
                        SelectionLimits::default()
                    ),
                    Err(SelectionError::AbsentField)
                );
            }
        }
    }
}

#[test]
fn transaction_byte_slices_share_nodes_and_respect_aggregate_limits() {
    let reference = transaction_reference();
    let transactions = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    let request = request(&[
        ("[0].signature", SelectionOperation::Slice { start: 30, end: 36 }),
        ("[0].signature", SelectionOperation::Slice { start: 31, end: 35 }),
        ("[0].signature", SelectionOperation::Slice { start: 30, end: 36 }),
    ]);
    let response = checked(ComponentSnapshot::Transactions(&transactions), &request);
    assert_eq!(response.selection.results[0], response.selection.results[2]);
    assert_eq!(
        &response.selection.results[0].values_ssz[0][1..5],
        response.selection.results[1].values_ssz[0].as_ref()
    );
    for limits in [
        SelectionLimits { targets: 13, ..SelectionLimits::default() },
        SelectionLimits { witness_hashes: 1, ..SelectionLimits::default() },
        SelectionLimits { source_bytes: 1, ..SelectionLimits::default() },
        SelectionLimits { response_bytes: 1, ..SelectionLimits::default() },
    ] {
        assert!(matches!(
            select_component(ComponentSnapshot::Transactions(&transactions), &request, limits),
            Err(SelectionError::LimitExceeded(_))
        ));
    }
    let mut corrupt = response;
    corrupt.selection.multiproof.as_mut().unwrap().targets.push(B256::ZERO);
    assert!(verify_component_selection(
        ComponentObject::Transactions,
        &request,
        &corrupt,
        transactions.root(),
        SelectionLimits::default()
    )
    .is_err());
}

#[test]
fn transaction_and_bal_component_links_verify_against_independent_payload_roots() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../block/tests/fixtures/payload.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let receipts = reth_pureth_receipt::eip6466::Eip6466ReceiptSnapshot::build(
            reth_pureth_receipt::eip6466::Receipts::from_ssz_bytes(&bytes(&case["receipts"]))
                .unwrap(),
        )
        .unwrap();
        let requests = ExecutionRequestsSnapshot::from_ssz(&bytes(&case["requests"])).unwrap();
        let payload = reth_pureth_block::ExecutionPayloadSnapshot::from_ssz(
            &bytes(&case["payload"]),
            receipts,
            requests,
            reth_pureth_block::PayloadLimits::default(),
        )
        .unwrap();
        for component in [
            ComponentSnapshot::Transactions(&payload.components().transactions),
            ComponentSnapshot::BlockAccessList(&payload.components().block_access_list),
        ] {
            let proof =
                link_component(component, payload.summary(), SelectionLimits::default()).unwrap();
            verify_component_link(
                component.object(),
                component.tree().root(),
                &proof,
                root(&case["root"]),
                SelectionLimits::default(),
            )
            .unwrap();
            assert!(verify_component_link(
                component.object(),
                B256::ZERO,
                &proof,
                root(&case["root"]),
                SelectionLimits::default()
            )
            .is_err());
        }
        let bal = &payload.components().block_access_list;
        let response = checked(
            ComponentSnapshot::BlockAccessList(bal),
            &request(&[
                (".", SelectionOperation::Whole {}),
                (".", SelectionOperation::Length {}),
                (".", SelectionOperation::Slice { start: 0, end: 1 }),
                ("[0]", SelectionOperation::Value {}),
            ]),
        );
        assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), bal.serialized());
    }
}

fn reference() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tests/fixtures/components.json")).unwrap()
}
fn bytes(value: &serde_json::Value) -> Bytes {
    value.as_str().unwrap().parse().unwrap()
}
fn root(value: &serde_json::Value) -> B256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn numeric_components_are_scalars_not_byte_collections() {
    let reference = reference();
    let summary =
        ExecutionSummarySnapshot::from_ssz(&bytes(&reference["summary"]["bytes"])).unwrap();
    let withdrawals =
        WithdrawalSnapshot::from_ssz(&bytes(&reference["withdrawals"]["bytes"])).unwrap();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    for (snapshot, paths) in [
        (
            ComponentSnapshot::ExecutionSummary(&summary),
            vec![
                ".number",
                ".timestamp",
                ".slot_number",
                ".gas_used.regular",
                ".gas_limits.blob",
                ".excess_gas.blob",
                ".base_fees_per_gas.regular",
                ".base_fees_per_gas.blob",
            ],
        ),
        (
            ComponentSnapshot::Withdrawals(&withdrawals),
            vec!["[0].index", "[0].validator_index", "[0].amount"],
        ),
        (
            ComponentSnapshot::ExecutionRequests(&requests),
            vec![".deposits[0].amount", ".deposits[0].index", ".withdrawals[0].amount"],
        ),
    ] {
        for path in paths {
            for operation in [
                SelectionOperation::Value {},
                SelectionOperation::Whole {},
                SelectionOperation::Presence {},
            ] {
                checked(snapshot, &request(&[(path, operation)]));
            }
            for operation in [
                SelectionOperation::Length {},
                SelectionOperation::Slice { start: 0, end: 1 },
                SelectionOperation::Range { start: 0, end: 1 },
            ] {
                assert_eq!(
                    select_component(
                        snapshot,
                        &request(&[(path, operation)]),
                        SelectionLimits::default()
                    ),
                    Err(SelectionError::UnsupportedOperation)
                );
            }
            assert_eq!(
                select_component(
                    snapshot,
                    &request(&[(&format!("{path}[0]"), SelectionOperation::Value {})]),
                    SelectionLimits::default()
                ),
                Err(SelectionError::InvalidPath)
            );
        }
    }
    checked(
        ComponentSnapshot::ExecutionSummary(&summary),
        &request(&[
            (".parent_hash[0]", SelectionOperation::Value {}),
            (".miner", SelectionOperation::Length {}),
        ]),
    );
}
fn request(paths: &[(&str, SelectionOperation)]) -> SelectionRequest {
    SelectionRequest {
        include_proof: true,
        selections: paths
            .iter()
            .map(|(path, operation)| ReceiptSelection {
                path: if path.starts_with('.') || path.starts_with('[') {
                    (*path).into()
                } else {
                    format!(".{path}")
                },
                operation: operation.clone(),
            })
            .collect(),
    }
}
fn checked(snapshot: ComponentSnapshot<'_>, request: &SelectionRequest) -> ComponentResponse {
    let response = select_component(snapshot, request, SelectionLimits::default()).unwrap();
    verify_component_selection(
        snapshot.object(),
        request,
        &response,
        snapshot.tree().root(),
        SelectionLimits::default(),
    )
    .unwrap();
    response
}

#[test]
fn component_codecs_roots_and_branches_match_the_independent_reference() {
    let reference = reference();
    let withdrawals =
        WithdrawalSnapshot::from_ssz(&bytes(&reference["withdrawals"]["bytes"])).unwrap();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let summary =
        ExecutionSummarySnapshot::from_ssz(&bytes(&reference["summary"]["bytes"])).unwrap();
    let empty =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["empty_requests"]["bytes"])).unwrap();
    assert_eq!(empty.root(), root(&reference["empty_requests"]["root"]));
    for (name, snapshot) in [
        ("withdrawals", ComponentSnapshot::Withdrawals(&withdrawals)),
        ("requests", ComponentSnapshot::ExecutionRequests(&requests)),
        ("summary", ComponentSnapshot::ExecutionSummary(&summary)),
    ] {
        assert_eq!(snapshot.tree().root(), root(&reference[name]["root"]));
        assert_eq!(snapshot.serialized(), bytes(&reference[name]["bytes"]).as_ref());
        for case in reference[name]["cases"].as_array().unwrap() {
            let index =
                u128::from_str_radix(case["gindex"].as_str().unwrap().trim_start_matches("0x"), 16)
                    .unwrap();
            let (node, branch) =
                crate::proof_access::node_and_branch(snapshot.tree(), index).unwrap();
            assert_eq!(node, root(&case["node"]));
            assert_eq!(
                branch,
                case["branch"].as_array().unwrap().iter().map(root).collect::<Vec<_>>()
            );
        }
        checked(snapshot, &request(&[(".", SelectionOperation::Whole {})]));
    }
}

#[test]
fn withdrawal_queries_cover_fields_whole_values_lengths_ranges_and_presence() {
    let reference = reference();
    let withdrawals =
        WithdrawalSnapshot::from_ssz(&bytes(&reference["withdrawals"]["bytes"])).unwrap();
    let snapshot = ComponentSnapshot::Withdrawals(&withdrawals);
    let request = request(&[
        ("[5].address", SelectionOperation::Value {}),
        (".", SelectionOperation::Length {}),
        (".", SelectionOperation::Range { start: 1, end: 6 }),
        ("[0]", SelectionOperation::Whole {}),
        ("[0].amount", SelectionOperation::Presence {}),
    ]);
    let response = checked(snapshot, &request);
    assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[6; 20]);
    assert_eq!(response.selection.results[1].values_ssz[0].as_ref(), &6_u64.to_le_bytes());
    assert_eq!(response.selection.results[2].values_ssz.len(), 5);
    assert_eq!(response.selection.results[3].values_ssz[0].len(), 44);
    assert_eq!(response.selection.results[4].values_ssz[0].as_ref(), &[1]);
    for index in 0..6 {
        for field in ["index", "validator_index", "address", "amount"] {
            checked(
                snapshot,
                &self::request(&[(&format!("[{index}].{field}"), SelectionOperation::Value {})]),
            );
        }
    }
}

#[test]
fn request_queries_bind_fixed_bytes_list_bounds_and_whole_requests() {
    let reference = reference();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let snapshot = ComponentSnapshot::ExecutionRequests(&requests);
    let request = request(&[
        ("deposits[0].pubkey", SelectionOperation::Whole {}),
        ("deposits[0].signature", SelectionOperation::Slice { start: 31, end: 65 }),
        ("withdrawals[0].amount", SelectionOperation::Value {}),
        ("consolidations", SelectionOperation::Range { start: 0, end: 1 }),
        ("deposits", SelectionOperation::Length {}),
        ("deposits[0].pubkey[47]", SelectionOperation::Value {}),
    ]);
    let response = checked(snapshot, &request);
    assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[1; 48]);
    assert_eq!(response.selection.results[1].values_ssz[0].as_ref(), &[4; 34]);
    assert_eq!(response.selection.results[2].values_ssz[0].as_ref(), &8_u64.to_le_bytes());
    assert_eq!(response.selection.results[3].values_ssz[0].len(), 116);
    assert_eq!(response.selection.results[4].values_ssz[0].as_ref(), &1_u64.to_le_bytes());
    assert_eq!(response.selection.results[5].values_ssz[0].as_ref(), &[1]);
}

#[test]
fn summary_queries_bind_active_fields_gas_fees_roots_extra_data_and_slot() {
    let reference = reference();
    let summary =
        ExecutionSummarySnapshot::from_ssz(&bytes(&reference["summary"]["bytes"])).unwrap();
    let snapshot = ComponentSnapshot::ExecutionSummary(&summary);
    let response = checked(
        snapshot,
        &request(&[
            ("gas_used.regular", SelectionOperation::Value {}),
            ("base_fees_per_gas.blob", SelectionOperation::Value {}),
            ("extra_data", SelectionOperation::Slice { start: 1, end: 3 }),
            ("slot_number", SelectionOperation::Value {}),
        ]),
    );
    assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &21_000_u64.to_le_bytes());
    assert_eq!(response.selection.results[1].values_ssz[0][0], 20);
    assert_eq!(response.selection.results[2].values_ssz[0].as_ref(), b"bc");
    assert_eq!(response.selection.results[3].values_ssz[0].as_ref(), &18_u64.to_le_bytes());
    for (field, _) in Record::Summary.fields() {
        checked(snapshot, &request(&[(field, SelectionOperation::Whole {})]));
    }
}

#[test]
fn component_verification_rejects_mutated_values_positions_roots_and_extra_evidence() {
    let reference = reference();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let snapshot = ComponentSnapshot::ExecutionRequests(&requests);
    let request = request(&[("deposits[0]", SelectionOperation::Whole {})]);
    let valid = checked(snapshot, &request);
    let mut changed = valid.clone();
    changed.selection.results[0].values_ssz[0] = Bytes::from(vec![0; 192]);
    assert!(verify_component_selection(
        snapshot.object(),
        &request,
        &changed,
        snapshot.tree().root(),
        SelectionLimits::default()
    )
    .is_err());
    let mut changed = valid.clone();
    changed.selection.multiproof.as_mut().unwrap().targets[0][0] ^= 1;
    assert!(verify_component_selection(
        snapshot.object(),
        &request,
        &changed,
        snapshot.tree().root(),
        SelectionLimits::default()
    )
    .is_err());
    let mut changed = valid.clone();
    changed.selection.multiproof.as_mut().unwrap().helpers.push(B256::ZERO);
    assert!(verify_component_selection(
        snapshot.object(),
        &request,
        &changed,
        snapshot.tree().root(),
        SelectionLimits::default()
    )
    .is_err());
    assert!(verify_component_selection(
        snapshot.object(),
        &request,
        &valid,
        B256::ZERO,
        SelectionLimits::default()
    )
    .is_err());
    assert!(verify_component_selection(
        ComponentObject::Withdrawals,
        &request,
        &valid,
        snapshot.tree().root(),
        SelectionLimits::default()
    )
    .is_err());
}

#[test]
fn component_bounds_and_cancellation_fail_without_partial_results() {
    let reference = reference();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let snapshot = ComponentSnapshot::ExecutionRequests(&requests);
    for (path, operation) in [
        ("deposits[1]", SelectionOperation::Whole {}),
        ("deposits[0].pubkey[48]", SelectionOperation::Value {}),
        ("deposits", SelectionOperation::Range { start: 0, end: 2 }),
    ] {
        assert_eq!(
            select_component(snapshot, &request(&[(path, operation)]), SelectionLimits::default()),
            Err(SelectionError::OutOfBounds)
        );
    }
    let request = request(&[(".", SelectionOperation::Whole {})]);
    assert_eq!(
        select_component_with_cancel(
            snapshot,
            &request,
            SelectionLimits::default(),
            &AtomicBool::new(true)
        ),
        Err(SelectionError::Cancelled)
    );
    assert_eq!(
        select_component(
            snapshot,
            &request,
            SelectionLimits { source_bytes: 1, ..Default::default() }
        ),
        Err(SelectionError::LimitExceeded("source bytes"))
    );
    assert_eq!(
        select_component(
            snapshot,
            &request,
            SelectionLimits { response_bytes: 1, ..Default::default() }
        ),
        Err(SelectionError::LimitExceeded("value bytes"))
    );
}

#[test]
fn component_links_bind_the_component_root_to_the_caller_expected_summary_root() {
    let reference = reference();
    let withdrawals =
        WithdrawalSnapshot::from_ssz(&bytes(&reference["withdrawals"]["bytes"])).unwrap();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let summary =
        ExecutionSummarySnapshot::from_ssz(&bytes(&reference["summary"]["bytes"])).unwrap();
    for component in [
        ComponentSnapshot::Withdrawals(&withdrawals),
        ComponentSnapshot::ExecutionRequests(&requests),
    ] {
        let proof = link_component(component, &summary, SelectionLimits::default()).unwrap();
        let response_bytes = serde_json::to_vec(&proof).unwrap().len();
        assert!(serde_json::to_vec(&proof.summary).unwrap().len() < response_bytes);
        for (response_bytes, accepted) in [(response_bytes, true), (response_bytes - 1, false)] {
            let limits = SelectionLimits { response_bytes, ..SelectionLimits::default() };
            let generated = link_component(component, &summary, limits);
            let verified = verify_component_link(
                component.object(),
                component.tree().root(),
                &proof,
                summary.root(),
                limits,
            );
            if accepted {
                assert_eq!(generated.unwrap(), proof);
                assert_eq!(verified, Ok(()));
            } else {
                assert_eq!(generated, Err(SelectionError::LimitExceeded("response bytes")));
                assert_eq!(verified, Err(SelectionError::LimitExceeded("response bytes")));
            }
        }
        verify_component_link(
            component.object(),
            component.tree().root(),
            &proof,
            summary.root(),
            SelectionLimits::default(),
        )
        .unwrap();
        assert!(verify_component_link(
            component.object(),
            B256::ZERO,
            &proof,
            summary.root(),
            SelectionLimits::default()
        )
        .is_err());
        assert!(verify_component_link(
            component.object(),
            component.tree().root(),
            &proof,
            B256::ZERO,
            SelectionLimits::default()
        )
        .is_err());
    }
    let wrong = WithdrawalSnapshot::build(Vec::new()).unwrap();
    assert!(link_component(
        ComponentSnapshot::Withdrawals(&wrong),
        &summary,
        SelectionLimits::default()
    )
    .is_err());
}

#[test]
fn component_codecs_reject_noncanonical_offsets_incomplete_records_and_bounds() {
    use reth_pureth_block::{
        ConsolidationRequest, DepositRequest, ExecutionRequests, WithdrawalRequest,
    };
    let reference = reference();
    let encoded = bytes(&reference["requests"]["bytes"]);
    for offset in [0_u32, 11, 13, u32::MAX] {
        let mut changed = encoded.to_vec();
        changed[..4].copy_from_slice(&offset.to_le_bytes());
        assert!(ExecutionRequests::from_ssz_bytes(&changed).is_err());
    }
    let mut changed = encoded.to_vec();
    changed.pop();
    assert!(ExecutionRequests::from_ssz_bytes(&changed).is_err());
    let encoded = bytes(&reference["summary"]["bytes"]);
    for offset in [0_u32, 447, 449, u32::MAX] {
        let mut changed = encoded.to_vec();
        changed[196..200].copy_from_slice(&offset.to_le_bytes());
        assert!(reth_pureth_block::ExecutionSummary::from_ssz_bytes(&changed).is_err());
    }
    let mut summary = reth_pureth_block::ExecutionSummary::from_ssz_bytes(&encoded).unwrap();
    summary.extra_data = Bytes::from(vec![0; 33]);
    assert!(summary.to_ssz_bytes().is_err());
    summary.extra_data = Bytes::new();
    summary.gas_used.blob = summary.gas_limits.blob + 1;
    assert!(summary.to_ssz_bytes().is_err());
    for requests in [
        ExecutionRequests { deposits: vec![DepositRequest::default(); 8193], ..Default::default() },
        ExecutionRequests {
            withdrawals: vec![WithdrawalRequest::default(); 17],
            ..Default::default()
        },
        ExecutionRequests {
            consolidations: vec![ConsolidationRequest::default(); 3],
            ..Default::default()
        },
    ] {
        assert!(requests.to_ssz_bytes().is_err());
        assert!(requests.tree().is_err());
    }
    let maximum = ExecutionRequests {
        deposits: vec![DepositRequest::default(); 8192],
        withdrawals: vec![WithdrawalRequest::default(); 16],
        consolidations: vec![ConsolidationRequest::default(); 2],
    };
    let encoded = maximum.to_ssz_bytes().unwrap();
    assert_eq!(ExecutionRequests::from_ssz_bytes(&encoded).unwrap(), maximum);
    assert_eq!(maximum.tree().unwrap().root(), root(&reference["maximum_requests"]["root"]));
    let maximum = ExecutionRequestsSnapshot::build(maximum).unwrap();
    checked(
        ComponentSnapshot::ExecutionRequests(&maximum),
        &request(&[(".", SelectionOperation::Whole {})]),
    );
}

#[test]
fn component_value_only_requests_remain_unverified_and_preserve_strict_path_grammar() {
    let reference = reference();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let snapshot = ComponentSnapshot::ExecutionRequests(&requests);
    let mut request = request(&[("deposits[0].pubkey", SelectionOperation::Whole {})]);
    request.include_proof = false;
    let response = select_component(snapshot, &request, SelectionLimits::default()).unwrap();
    assert!(response.selection.multiproof.is_none());
    assert_eq!(
        verify_component_selection(
            snapshot.object(),
            &request,
            &response,
            requests.root(),
            SelectionLimits::default()
        ),
        Err(SelectionError::Unverified)
    );
    request.selections[0].path = "deposits[0].pubkey".into();
    assert_eq!(
        select_component(snapshot, &request, SelectionLimits::default()),
        Err(SelectionError::InvalidPath)
    );
}

#[test]
fn empty_component_lists_authenticate_lengths_and_empty_results() {
    let withdrawals = WithdrawalSnapshot::build(Vec::new()).unwrap();
    let requests = ExecutionRequestsSnapshot::build(Default::default()).unwrap();
    for (snapshot, path) in [
        (ComponentSnapshot::Withdrawals(&withdrawals), "."),
        (ComponentSnapshot::ExecutionRequests(&requests), ".deposits"),
        (ComponentSnapshot::ExecutionRequests(&requests), ".withdrawals"),
        (ComponentSnapshot::ExecutionRequests(&requests), ".consolidations"),
    ] {
        let response = checked(
            snapshot,
            &request(&[
                (path, SelectionOperation::Length {}),
                (path, SelectionOperation::Range { start: 0, end: 0 }),
                (path, SelectionOperation::Whole {}),
            ]),
        );
        assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &0_u64.to_le_bytes());
        assert!(response.selection.results[1].values_ssz.is_empty());
        assert!(response.selection.results[2].values_ssz[0].is_empty());
        assert!(!response.selection.multiproof.unwrap().targets.is_empty());
    }
}

#[test]
fn component_server_proofs_verify_under_the_same_constrained_source_budget() {
    let reference = reference();
    let withdrawals =
        WithdrawalSnapshot::from_ssz(&bytes(&reference["withdrawals"]["bytes"])).unwrap();
    let requests =
        ExecutionRequestsSnapshot::from_ssz(&bytes(&reference["requests"]["bytes"])).unwrap();
    let summary =
        ExecutionSummarySnapshot::from_ssz(&bytes(&reference["summary"]["bytes"])).unwrap();
    for (snapshot, paths) in [
        (
            ComponentSnapshot::Withdrawals(&withdrawals),
            vec![
                (".", SelectionOperation::Whole {}),
                (".", SelectionOperation::Range { start: 0, end: 6 }),
                ("[0].address", SelectionOperation::Value {}),
            ],
        ),
        (
            ComponentSnapshot::ExecutionRequests(&requests),
            vec![
                (".", SelectionOperation::Whole {}),
                (".deposits[0].signature", SelectionOperation::Slice { start: 31, end: 65 }),
                (".deposits[0].pubkey", SelectionOperation::Value {}),
            ],
        ),
        (
            ComponentSnapshot::ExecutionSummary(&summary),
            vec![
                (".", SelectionOperation::Whole {}),
                (".extra_data", SelectionOperation::Slice { start: 0, end: 3 }),
                (".gas_used", SelectionOperation::Whole {}),
            ],
        ),
    ] {
        let request = request(&paths);
        let mut successes = 0;
        for source_bytes in [1024, 4096, 16384, 32768, 65536, 131072, 1048576, 64 * 1048576] {
            let limits = SelectionLimits { source_bytes, ..Default::default() };
            match select_component(snapshot, &request, limits) {
                Ok(response) => {
                    verify_component_selection(
                        snapshot.object(),
                        &request,
                        &response,
                        snapshot.tree().root(),
                        limits,
                    )
                    .unwrap();
                    successes += 1;
                }
                Err(SelectionError::LimitExceeded("source bytes")) => {}
                other => panic!("unexpected budget result: {other:?}"),
            }
        }
        assert!(successes > 0);
    }
}

#[test]
fn fixed_byte_vectors_reject_nonzero_chunk_and_byte_padding() {
    let target = Target { index: 1, kind: Kind::Bytes(96), length: None };
    assert_eq!(value(target, &mut |_| Ok(B256::ZERO)).unwrap().len(), 96);
    assert_eq!(
        value(target, &mut |index| Ok(if index == 7 { B256::repeat_byte(1) } else { B256::ZERO })),
        Err(SelectionError::InvalidValue)
    );
    let target = Target { index: 1, kind: Kind::Bytes(48), length: None };
    assert_eq!(
        value(target, &mut |index| Ok(if index == 3 { B256::repeat_byte(1) } else { B256::ZERO })),
        Err(SelectionError::InvalidValue)
    );
}

#[test]
fn component_byte_materialization_checks_remaining_budget_before_later_selections() {
    let reference = transaction_reference();
    let originals = TransactionSnapshot::from_ssz(&bytes(&reference["bytes"])).unwrap();
    let transaction = &originals.transactions()[0];
    let TransactionPayload::Legacy(mut payload) = transaction.payload().clone() else {
        unreachable!()
    };
    let length = 65_536;
    payload.input = Bytes::from(vec![1; length]);
    let transactions = TransactionSnapshot::build(vec![TransactionSsz::new(
        TransactionPayload::Legacy(payload),
        *transaction.signature(),
    )
    .unwrap()])
    .unwrap();
    let mut nodes = 0;
    let mut pending = vec![transactions.tree()];
    while let Some(node) = pending.pop() {
        nodes += 1;
        if let Some(children) = node.children() {
            pending.extend(children);
        }
    }
    let source = transactions.serialized().len() * 3 + 256 + nodes * size_of::<RetainedNode>();
    for operation in [
        SelectionOperation::Whole {},
        SelectionOperation::Value {},
        SelectionOperation::Slice { start: 0, end: 512 },
        SelectionOperation::Range { start: 0, end: 512 },
    ] {
        let selected = match operation {
            SelectionOperation::Slice { start, end } | SelectionOperation::Range { start, end } => {
                (end - start) as usize
            }
            _ => length,
        };
        for earlier in [false, true] {
            let mut request = request(&[
                ("[0].payload.input_", operation.clone()),
                (".invalid", SelectionOperation::Value {}),
            ]);
            request.include_proof = false;
            if earlier {
                request.selections.insert(
                    0,
                    ReceiptSelection {
                        path: "[0].payload.nonce".into(),
                        operation: SelectionOperation::Value {},
                    },
                );
            }
            let limits = SelectionLimits {
                source_bytes: source + if earlier { 2 * selected + 15 } else { 1 },
                ..Default::default()
            };
            assert_eq!(
                select_component(ComponentSnapshot::Transactions(&transactions), &request, limits),
                Err(SelectionError::LimitExceeded("source bytes"))
            );
        }
    }
}
