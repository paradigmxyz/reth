use super::*;
use alloy_primitives::{b256, Address};
use reth_pureth_receipt::eip6466::{BasicReceipt, CreateReceipt, Log, Receipts, SetCodeReceipt};

#[test]
fn whole_bytes_match_independent_offset_encoding() {
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/receipt_selection.json")).unwrap();
    let snapshot = fixture();
    assert_eq!(alloy_primitives::hex::encode(snapshot.serialized()), reference["list_bytes"]);
    for (index, expected) in reference["receipt_bytes"].as_array().unwrap().iter().enumerate() {
        let response =
            run(&snapshot, &single_request(&format!("[{index}]"), SelectionOperation::Whole {}));
        assert_eq!(
            alloy_primitives::hex::encode(&response.results[0].values_ssz[0]),
            expected.as_str().unwrap()
        );
    }
    for (path, field) in [("[0].logs[0]", "log_bytes"), ("[0].logs", "logs_bytes")] {
        let response = run(&snapshot, &single_request(path, SelectionOperation::Whole {}));
        assert_eq!(
            alloy_primitives::hex::encode(&response.results[0].values_ssz[0]),
            reference[field]
        );
    }
}

#[test]
fn server_does_not_emit_whole_objects_above_the_verifier_materialization_limit() {
    let snapshot = fixture();
    let request = single_request(".", SelectionOperation::Whole {});
    let source =
        source_budget(&snapshot, SelectionLimits::default(), &AtomicBool::new(false)).unwrap();
    let mut required = snapshot.serialized().len() * 8 + 4096;
    whole_structure_budget(Kind::Receipts, snapshot.serialized(), &mut required, usize::MAX)
        .unwrap();
    let server_limits = SelectionLimits {
        source_bytes: source + snapshot.serialized().len() * 2 + required - 1,
        ..Default::default()
    };
    assert!(source_budget(&snapshot, server_limits, &AtomicBool::new(false)).is_ok());
    assert_eq!(
        select_receipt_snapshot(&snapshot, &request, server_limits),
        Err(SelectionError::LimitExceeded("source bytes"))
    );
    let response = run(&snapshot, &request);
    assert_eq!(
        verify_receipt_selection(
            &request,
            &response,
            snapshot.root(),
            SelectionLimits { source_bytes: required - 1, ..Default::default() }
        ),
        Err(SelectionError::LimitExceeded("source bytes"))
    );
}

fn fixture() -> Eip6466ReceiptSnapshot {
    let log = Log::new(
        Address::repeat_byte(0x44),
        vec![B256::repeat_byte(0x55), B256::repeat_byte(0x66)],
        Bytes::from((0..33).collect::<Vec<u8>>()),
    )
    .unwrap();
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(vec![
        Receipt::Basic(BasicReceipt {
            from_: Address::repeat_byte(0x11),
            gas_used: 21_000,
            logs: vec![log.clone()],
            status: true,
        }),
        Receipt::Create(CreateReceipt {
            from_: Address::repeat_byte(0x22),
            gas_used: 42_000,
            contract_address: Address::ZERO,
            logs: vec![log.clone()],
            status: false,
        }),
        Receipt::SetCode(SetCodeReceipt {
            from_: Address::repeat_byte(0x33),
            gas_used: 63_000,
            logs: vec![log],
            status: false,
            authorities: vec![
                Address::repeat_byte(0x77),
                Address::ZERO,
                Address::repeat_byte(0x77),
            ],
        }),
    ]))
    .unwrap();
    assert_eq!(
        snapshot.root(),
        b256!("520f710a6fb26f151f43c40cb9a17ee40b93c8dcd3e02be2ba5e266510fa4308")
    );
    snapshot
}

fn single_request(path: &str, operation: SelectionOperation) -> SelectionRequest {
    SelectionRequest {
        selections: vec![ReceiptSelection { path: path.to_owned(), operation }],
        include_proof: true,
    }
}

fn run(snapshot: &Eip6466ReceiptSnapshot, request: &SelectionRequest) -> SelectionResponse {
    let response = select_receipt_snapshot(snapshot, request, SelectionLimits::default()).unwrap();
    assert_eq!(
        verify_receipt_selection(request, &response, snapshot.root(), SelectionLimits::default()),
        Ok(())
    );
    response
}

#[test]
fn every_present_scalar_field_uses_the_eip_tree() {
    let snapshot = fixture();
    for (index, sender, gas, status) in
        [(0, 0x11, 21_000_u64, 1), (1, 0x22, 42_000, 0), (2, 0x33, 63_000, 0)]
    {
        for (suffix, expected) in [
            ("from", vec![sender; 20]),
            ("gas_used", gas.to_le_bytes().to_vec()),
            ("status", vec![status]),
            ("logs[0].address", vec![0x44; 20]),
            ("logs[0].topics[1]", vec![0x66; 32]),
            ("logs[0].data[32]", vec![32]),
        ] {
            let response = run(
                &snapshot,
                &single_request(&format!("[{index}].{suffix}"), SelectionOperation::Value {}),
            );
            assert_eq!(response.results[0].values_ssz, vec![Bytes::from(expected)]);
        }
    }
    assert_eq!(
        run(&snapshot, &single_request("[1].contract_address", SelectionOperation::Value {}))
            .results[0]
            .values_ssz,
        vec![Bytes::from(vec![0; 20])]
    );
    assert_eq!(
        run(&snapshot, &single_request("[2].authorities[1]", SelectionOperation::Value {})).results
            [0]
        .values_ssz,
        vec![Bytes::from(vec![0; 20])]
    );
}

#[test]
fn field_positions_and_context_match_pinned_reference_indices() {
    let snapshot = fixture();
    let cases: &[(&str, u128, &[u128])] = &[
        ("[0].from", 32, &[3, 9, 17]),
        ("[0].gas_used", 264, &[3, 9, 17]),
        ("[0].status", 267, &[3, 9, 17]),
        ("[0].logs[0].address", 4256, &[3, 9, 17, 533]),
        ("[0].logs[0].topics[1]", 34057, &[3, 9, 17, 533, 8515]),
        ("[0].logs[0].data[0]", 17032, &[3, 9, 17, 533, 8517, 136264]),
        ("[0].logs[0].data[31]", 17032, &[3, 9, 17, 533, 8517, 136264]),
        ("[0].logs[0].data[32]", 136264, &[3, 9, 17, 533, 8517, 136264]),
        ("[1].from", 320, &[3, 81, 161]),
        ("[1].gas_used", 2568, &[3, 81, 161]),
        ("[1].status", 2571, &[3, 81, 161]),
        ("[1].logs[0].address", 41120, &[3, 81, 161, 5141]),
        ("[1].logs[0].topics[1]", 328969, &[3, 81, 161, 5141, 82243]),
        ("[1].logs[0].data[0]", 164488, &[3, 81, 161, 5141, 82245, 1315912]),
        ("[1].logs[0].data[31]", 164488, &[3, 81, 161, 5141, 82245, 1315912]),
        ("[1].logs[0].data[32]", 1315912, &[3, 81, 161, 5141, 82245, 1315912]),
        ("[2].from", 328, &[3, 83, 165]),
        ("[2].gas_used", 2632, &[3, 83, 165]),
        ("[2].status", 2635, &[3, 83, 165]),
        ("[2].logs[0].address", 42144, &[3, 83, 165, 5269]),
        ("[2].logs[0].topics[1]", 337161, &[3, 83, 165, 5269, 84291]),
        ("[2].logs[0].data[0]", 168584, &[3, 83, 165, 5269, 84293, 1348680]),
        ("[2].logs[0].data[31]", 168584, &[3, 83, 165, 5269, 84293, 1348680]),
        ("[2].logs[0].data[32]", 1348680, &[3, 83, 165, 5269, 84293, 1348680]),
        ("[1].contract_address", 2569, &[3, 81, 161]),
        ("[2].authorities[0]", 84352, &[3, 83, 165, 42177]),
        ("[2].authorities[1]", 674824, &[3, 83, 165, 42177]),
        ("[2].authorities[2]", 674825, &[3, 83, 165, 42177]),
    ];
    for &(path, expected_index, context) in cases {
        let mut positions = Vec::new();
        let target = resolve_eip(&tokens(path).unwrap(), &mut |index| {
            positions.push(index);
            Ok(node_and_branch(snapshot.tree(), index).unwrap().0)
        })
        .unwrap();
        assert_eq!(target.index, expected_index, "{path}");
        assert_eq!(positions, context, "{path}");

        let response = run(&snapshot, &single_request(path, SelectionOperation::Value {}));
        if !positions.contains(&expected_index) {
            positions.push(expected_index);
        }
        let witnesses = response.results[0].witnesses.as_ref().unwrap();
        assert_eq!(witnesses.len(), positions.len(), "{path}");
        for (witness, index) in witnesses.iter().zip(positions) {
            assert_eq!(
                verify_branch(witness.node, index, &witness.branch, snapshot.root()),
                Ok(())
            );
        }
    }
}

#[test]
fn absence_variant_and_lengths_are_authenticated() {
    let snapshot = fixture();
    for (index, selector) in [(0, 1), (1, 2), (2, 3)] {
        assert_eq!(
            run(&snapshot, &single_request(&format!("[{index}]"), SelectionOperation::Variant {}))
                .results[0]
                .values_ssz[0]
                .as_ref(),
            &[selector]
        );
    }
    for (path, present) in [
        ("[0].contract_address", 0),
        ("[1].contract_address", 1),
        ("[2].authorities", 1),
        ("[0].authorities", 0),
    ] {
        assert_eq!(
            run(&snapshot, &single_request(path, SelectionOperation::Presence {})).results[0]
                .values_ssz[0]
                .as_ref(),
            &[present]
        );
    }
    for (path, count) in [
        (".", 3_u64),
        ("[0].logs", 1),
        ("[0].logs[0].topics", 2),
        ("[0].logs[0].data", 33),
        ("[2].authorities", 3),
    ] {
        assert_eq!(
            run(&snapshot, &single_request(path, SelectionOperation::Length {})).results[0]
                .values_ssz[0]
                .as_ref(),
            &count.to_le_bytes()
        );
    }
    assert_eq!(
        select_receipt_snapshot(
            &snapshot,
            &single_request("[0].contract_address", SelectionOperation::Value {}),
            SelectionLimits::default()
        ),
        Err(SelectionError::AbsentField)
    );
    assert_eq!(
        select_receipt_snapshot(
            &snapshot,
            &single_request("[0].missing", SelectionOperation::Presence {}),
            SelectionLimits::default()
        ),
        Err(SelectionError::InvalidPath)
    );
}

#[test]
fn ranges_slices_and_whole_fixed_element_lists_verify() {
    let snapshot = fixture();
    let response = run(
        &snapshot,
        &single_request("[0].logs[0].data", SelectionOperation::Slice { start: 31, end: 33 }),
    );
    assert_eq!(response.results[0].values_ssz, vec![Bytes::from(vec![31, 32])]);
    assert_eq!(
        run(
            &snapshot,
            &single_request("[2].authorities", SelectionOperation::Range { start: 1, end: 3 })
        )
        .results[0]
            .values_ssz,
        vec![Bytes::from(vec![0; 20]), Bytes::from(vec![0x77; 20])]
    );
    assert_eq!(
        run(
            &snapshot,
            &single_request("[0].logs[0].topics", SelectionOperation::Range { start: 0, end: 2 })
        )
        .results[0]
            .values_ssz
            .len(),
        2
    );
    for path in ["[0].logs[0].data", "[0].logs[0].topics", "[2].authorities"] {
        run(&snapshot, &single_request(path, SelectionOperation::Whole {}));
    }
    let empty = run(
        &snapshot,
        &single_request("[0].logs[0].data", SelectionOperation::Range { start: 33, end: 33 }),
    );
    assert!(empty.results[0].values_ssz.is_empty());
    assert!(!empty.results[0].witnesses.as_ref().unwrap().is_empty());
    for (start, end) in [(34, 34), (4, 3), (0, 34)] {
        assert_eq!(
            select_receipt_snapshot(
                &snapshot,
                &single_request("[0].logs[0].data", SelectionOperation::Slice { start, end }),
                SelectionLimits::default()
            ),
            Err(SelectionError::OutOfBounds)
        );
    }
}

#[test]
fn multiple_selections_reject_semantic_overlap_but_allow_shared_chunks() {
    let snapshot = fixture();
    let request = SelectionRequest {
        selections: vec![
            ReceiptSelection { path: "[0].from".into(), operation: SelectionOperation::Value {} },
            ReceiptSelection { path: "[0].status".into(), operation: SelectionOperation::Value {} },
            ReceiptSelection {
                path: "[0].logs[0].data[1]".into(),
                operation: SelectionOperation::Value {},
            },
            ReceiptSelection {
                path: "[0].logs[0].data[2]".into(),
                operation: SelectionOperation::Value {},
            },
        ],
        include_proof: true,
    };
    let mut response = run(&snapshot, &request);
    response.results.swap(0, 1);
    assert!(verify_receipt_selection(
        &request,
        &response,
        snapshot.root(),
        SelectionLimits::default()
    )
    .is_err());
    let mut duplicate = request.clone();
    duplicate.selections.push(duplicate.selections[0].clone());
    assert_eq!(
        select_receipt_snapshot(&snapshot, &duplicate, SelectionLimits::default()),
        Err(SelectionError::DuplicateOrOverlap)
    );
    let mut overlap = request;
    overlap.selections.push(ReceiptSelection {
        path: "[0].logs[0].data".into(),
        operation: SelectionOperation::Whole {},
    });
    assert_eq!(
        select_receipt_snapshot(&snapshot, &overlap, SelectionLimits::default()),
        Err(SelectionError::DuplicateOrOverlap)
    );
}

#[test]
fn duplicate_paths_with_different_operations_are_rejected() {
    let snapshot = fixture();
    let mut request = SelectionRequest {
        selections: vec![
            ReceiptSelection {
                path: "[0].logs[0].data".into(),
                operation: SelectionOperation::Whole {},
            },
            ReceiptSelection {
                path: "[0].logs[0].data".into(),
                operation: SelectionOperation::Length {},
            },
        ],
        include_proof: true,
    };
    for include_proof in [true, false] {
        request.include_proof = include_proof;
        assert_eq!(
            select_receipt_snapshot(&snapshot, &request, SelectionLimits::default()),
            Err(SelectionError::DuplicateOrOverlap)
        );
    }
}

#[test]
fn mutation_missing_extra_and_reordered_evidence_fail() {
    let snapshot = fixture();
    let request = single_request("[0].logs[0].data[32]", SelectionOperation::Value {});
    let valid = run(&snapshot, &request);
    for witness in 0..valid.results[0].witnesses.as_ref().unwrap().len() {
        let mut changed = valid.clone();
        changed.results[0].witnesses.as_mut().unwrap()[witness].node[0] ^= 1;
        assert!(verify_receipt_selection(
            &request,
            &changed,
            snapshot.root(),
            SelectionLimits::default()
        )
        .is_err());
        for sibling in 0..valid.results[0].witnesses.as_ref().unwrap()[witness].branch.len() {
            let mut changed = valid.clone();
            changed.results[0].witnesses.as_mut().unwrap()[witness].branch[sibling][0] ^= 1;
            assert!(verify_receipt_selection(
                &request,
                &changed,
                snapshot.root(),
                SelectionLimits::default()
            )
            .is_err());
        }
    }
    let mut changed = valid.clone();
    changed.results[0].values_ssz[0] = Bytes::from(vec![0]);
    assert_eq!(
        verify_receipt_selection(&request, &changed, snapshot.root(), SelectionLimits::default()),
        Err(SelectionError::InvalidValue)
    );
    let mut changed = valid.clone();
    changed.results[0].witnesses.as_mut().unwrap().pop();
    assert!(verify_receipt_selection(
        &request,
        &changed,
        snapshot.root(),
        SelectionLimits::default()
    )
    .is_err());
    let mut changed = valid.clone();
    let extra = changed.results[0].witnesses.as_ref().unwrap()[0].clone();
    changed.results[0].witnesses.as_mut().unwrap().push(extra);
    assert!(verify_receipt_selection(
        &request,
        &changed,
        snapshot.root(),
        SelectionLimits::default()
    )
    .is_err());
    let mut changed = valid.clone();
    changed.results[0].witnesses.as_mut().unwrap().swap(0, 1);
    assert!(verify_receipt_selection(
        &request,
        &changed,
        snapshot.root(),
        SelectionLimits::default()
    )
    .is_err());
    assert_eq!(
        verify_receipt_selection(&request, &valid, B256::ZERO, SelectionLimits::default()),
        Err(SelectionError::InvalidContext)
    );
    let mut changed = valid.clone();
    changed.proof_format = None;
    assert_eq!(
        verify_receipt_selection(&request, &changed, snapshot.root(), SelectionLimits::default()),
        Err(SelectionError::InvalidContext)
    );
    let mut changed = valid;
    changed.results[0].witnesses = None;
    assert_eq!(
        verify_receipt_selection(&request, &changed, snapshot.root(), SelectionLimits::default()),
        Err(SelectionError::InvalidProof)
    );
}

#[test]
fn successful_create_empty_authorities_and_four_topics_verify() {
    let log =
        Log::new(Address::repeat_byte(1), (1..=4).map(B256::repeat_byte).collect(), Bytes::new())
            .unwrap();
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(vec![
        Receipt::Create(CreateReceipt {
            from_: Address::repeat_byte(2),
            gas_used: 1,
            contract_address: Address::repeat_byte(3),
            logs: vec![log],
            status: true,
        }),
        Receipt::SetCode(SetCodeReceipt {
            from_: Address::repeat_byte(4),
            gas_used: 1,
            logs: vec![],
            status: true,
            authorities: vec![],
        }),
    ]))
    .unwrap();
    assert_eq!(
        run(&snapshot, &single_request("[0].contract_address", SelectionOperation::Value {}))
            .results[0]
            .values_ssz[0],
        Bytes::from(vec![3; 20])
    );
    assert_eq!(
        run(&snapshot, &single_request("[0].logs[0].topics", SelectionOperation::Whole {})).results
            [0]
        .values_ssz[0]
            .len(),
        128
    );
    for operation in [SelectionOperation::Whole {}, SelectionOperation::Range { start: 0, end: 0 }]
    {
        let result = run(&snapshot, &single_request("[1].authorities", operation));
        assert!(result.results[0].values_ssz.iter().all(|value| value.is_empty()));
    }
    assert_eq!(
        select_receipt_snapshot(
            &snapshot,
            &single_request("[0].logs[0].topics[4]", SelectionOperation::Value {}),
            SelectionLimits::default()
        ),
        Err(SelectionError::OutOfBounds)
    );
}

#[test]
fn whole_objects_and_collections_use_canonical_codecs_and_reject_changed_bytes() {
    let snapshot = fixture();
    let request = single_request("[0].logs[0].data", SelectionOperation::Whole {});
    let mut response = run(&snapshot, &request);
    response.results[0].values_ssz[0] = Bytes::from(vec![0; 33]);
    assert_eq!(
        verify_receipt_selection(&request, &response, snapshot.root(), SelectionLimits::default()),
        Err(SelectionError::InvalidValue)
    );
    for path in [".", "[0]", "[1]", "[2]", "[0].logs", "[1].logs", "[2].logs", "[0].logs[0]"] {
        let request = single_request(path, SelectionOperation::Whole {});
        let response = run(&snapshot, &request);
        let value = &response.results[0].values_ssz[0];
        if path == "." {
            assert_eq!(value.as_ref(), snapshot.serialized());
        } else if path == "[0]" {
            assert_eq!(*value, snapshot.receipts().get(0).unwrap().to_ssz_bytes().unwrap());
        } else if path == "[0].logs[0]" {
            assert_eq!(
                *value,
                snapshot.receipts().get(0).unwrap().logs()[0].to_ssz_bytes().unwrap()
            );
        }
        let mut changed = response.clone();
        let mut bytes = value.to_vec();
        bytes[0] ^= 1;
        changed.results[0].values_ssz[0] = Bytes::from(bytes);
        assert!(verify_receipt_selection(
            &request,
            &changed,
            snapshot.root(),
            SelectionLimits::default()
        )
        .is_err());
        let mut changed = response;
        changed.results[0].values_ssz[0] = Bytes::from_static(&[0xff]);
        assert!(verify_receipt_selection(
            &request,
            &changed,
            snapshot.root(),
            SelectionLimits::default()
        )
        .is_err());
    }
}

#[test]
fn object_ranges_return_individually_encoded_values_and_bind_each_position() {
    let snapshot = fixture();
    for (path, end) in [(".", 3), ("[0].logs", 1)] {
        let request = single_request(path, SelectionOperation::Range { start: 0, end });
        let response = run(&snapshot, &request);
        assert_eq!(response.results[0].values_ssz.len(), end as usize);
        let mut changed = response.clone();
        changed.results[0].values_ssz[0] = Bytes::from_static(&[0xff]);
        assert!(verify_receipt_selection(
            &request,
            &changed,
            snapshot.root(),
            SelectionLimits::default()
        )
        .is_err());
        if end > 1 {
            let mut changed = response;
            changed.results[0].values_ssz.swap(0, 1);
            assert!(verify_receipt_selection(
                &request,
                &changed,
                snapshot.root(),
                SelectionLimits::default()
            )
            .is_err());
        }
    }
    let response =
        run(&snapshot, &single_request(".", SelectionOperation::Range { start: 3, end: 3 }));
    assert!(response.results[0].values_ssz.is_empty());
}

#[test]
fn value_only_is_explicitly_unverified_and_still_bounded() {
    let snapshot = fixture();
    let mut request = single_request("[0].from", SelectionOperation::Value {});
    request.include_proof = false;
    let response =
        select_receipt_snapshot(&snapshot, &request, SelectionLimits::default()).unwrap();
    assert!(response.proof_format.is_none());
    assert!(response.results[0].witnesses.is_none());
    assert_eq!(
        verify_receipt_selection(&request, &response, snapshot.root(), SelectionLimits::default()),
        Err(SelectionError::Unverified)
    );
    assert_eq!(
        select_receipt_snapshot(
            &snapshot,
            &request,
            SelectionLimits { targets: 0, ..Default::default() }
        ),
        Err(SelectionError::LimitExceeded("targets"))
    );
}

#[test]
fn every_independent_budget_rejects_the_first_excess() {
    let snapshot = fixture();
    let request = single_request("[0].from", SelectionOperation::Value {});
    let response = run(&snapshot, &request);
    let hashes = response.results[0]
        .witnesses
        .as_ref()
        .unwrap()
        .iter()
        .map(|witness| witness.branch.len() + 1)
        .sum();
    let bytes = serde_json::to_vec(&response).unwrap().len();
    assert!(select_receipt_snapshot(
        &snapshot,
        &request,
        SelectionLimits { witness_hashes: hashes, response_bytes: bytes, ..Default::default() }
    )
    .is_ok());
    for (limits, reason) in [
        (SelectionLimits { selections: 0, ..Default::default() }, "selections"),
        (SelectionLimits { request_bytes: 1, ..Default::default() }, "request bytes"),
        (SelectionLimits { targets: 0, ..Default::default() }, "targets"),
        (SelectionLimits { witness_hashes: hashes - 1, ..Default::default() }, "witness hashes"),
        (SelectionLimits { response_bytes: bytes - 1, ..Default::default() }, "response bytes"),
        (SelectionLimits { source_bytes: 0, ..Default::default() }, "source bytes"),
        (SelectionLimits { receipts: 2, ..Default::default() }, "receipts"),
        (SelectionLimits { logs: 2, ..Default::default() }, "logs"),
    ] {
        assert_eq!(
            select_receipt_snapshot(&snapshot, &request, limits),
            Err(SelectionError::LimitExceeded(reason))
        );
    }
    assert_eq!(
        select_receipt_snapshot(
            &snapshot,
            &single_request("[0].logs[0].data", SelectionOperation::Slice { start: 0, end: 33 }),
            SelectionLimits { targets: 32, ..Default::default() }
        ),
        Err(SelectionError::LimitExceeded("targets"))
    );
}

#[test]
fn finite_json_shapes_and_raw_request_limit_are_enforced() {
    let request = single_request("[0].from", SelectionOperation::Value {});
    let json = serde_json::to_vec(&request).unwrap();
    assert_eq!(decode_selection_request(&json, SelectionLimits::default()), Ok(request));
    assert_eq!(
        decode_selection_request(&vec![b' '; 65_537], SelectionLimits::default()),
        Err(SelectionError::LimitExceeded("request bytes"))
    );
    assert!(decode_selection_request(br#"{"selections":[{"path":"[0].from","operation":{"kind":"value","extra":1}}],"include_proof":true}"#, SelectionLimits::default()).is_err());
}

#[test]
fn selection_wire_has_no_receipt_schema_or_revision_dispatch() {
    let snapshot = fixture();
    let request = single_request("[0].from", SelectionOperation::Value {});
    let response = run(&snapshot, &request);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["proof_format"], "single_branch");
    assert_eq!(json.as_object().unwrap().len(), 3);
    assert_eq!(serde_json::from_value::<SelectionResponse>(json.clone()).unwrap(), response);

    for field in ["schema_id", "producer_revision", "version", "revision"] {
        let mut changed = json.clone();
        changed[field] = serde_json::json!("legacy");
        assert!(serde_json::from_value::<SelectionResponse>(changed).is_err());
        let mut changed = serde_json::to_value(&request).unwrap();
        changed[field] = serde_json::json!("legacy");
        assert!(decode_selection_request(
            &serde_json::to_vec(&changed).unwrap(),
            SelectionLimits::default()
        )
        .is_err());
    }
    for format in ["ssz_single_branch_v1", "multiproof", "pureth-receipt-v0"] {
        let mut changed = json.clone();
        changed["proof_format"] = serde_json::json!(format);
        assert!(serde_json::from_value::<SelectionResponse>(changed).is_err());
    }
    let mut value_request = request;
    value_request.include_proof = false;
    let value_response =
        select_receipt_snapshot(&snapshot, &value_request, SelectionLimits::default()).unwrap();
    let json = serde_json::to_value(value_response).unwrap();
    assert!(json["proof_format"].is_null());
    assert!(json["results"][0]["witnesses"].is_null());
}

#[test]
fn typed_layout_rejects_invalid_selectors_masks_and_lengths() {
    for (selector, mask) in [(0, 0x1b), (4, 0x1b), (1, 0x1f), (2, 0x1b), (3, 0x1b)] {
        let mut read = |index| {
            Ok(match index {
                3 => B256::right_padding_from(&1_u64.to_le_bytes()),
                9 => B256::right_padding_from(&[selector]),
                17 => B256::right_padding_from(&[mask]),
                _ => B256::ZERO,
            })
        };
        assert_eq!(
            resolve_eip(&tokens("[0].from").unwrap(), &mut read).err(),
            Some(SelectionError::InvalidContext)
        );
    }
    for index in [3, 9, 17] {
        let mut read = |position| {
            let mut node = match position {
                3 => B256::right_padding_from(&1_u64.to_le_bytes()),
                9 => B256::right_padding_from(&[1]),
                17 => B256::right_padding_from(&[0x1b]),
                _ => B256::ZERO,
            };
            if position == index {
                node[31] = 1;
            }
            Ok(node)
        };
        assert_eq!(
            resolve_eip(&tokens("[0].from").unwrap(), &mut read).err(),
            Some(SelectionError::InvalidValue)
        );
    }
}

#[test]
fn empty_lists_still_prove_bounds_and_invalid_context_is_rejected() {
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(vec![])).unwrap();
    let result =
        run(&snapshot, &single_request(".", SelectionOperation::Range { start: 0, end: 0 }));
    assert!(result.results[0].values_ssz.is_empty());
    assert_eq!(
        select_receipt_snapshot(
            &snapshot,
            &single_request("[0].from", SelectionOperation::Value {}),
            SelectionLimits::default()
        ),
        Err(SelectionError::OutOfBounds)
    );
    let mut read = |index| {
        Ok(if index == 3 { B256::right_padding_from(&1_u64.to_le_bytes()) } else { B256::ZERO })
    };
    assert_eq!(
        resolve_eip(&tokens("[0].from").unwrap(), &mut read).err(),
        Some(SelectionError::InvalidContext)
    );
}

#[test]
fn byte_boundaries_empty_values_and_progressive_positions_remain_valid() {
    for size in [0, 1, 31, 32, 33, 65] {
        let log = Log::new(Address::repeat_byte(1), vec![], Bytes::from(vec![0xab; size])).unwrap();
        let snapshot =
            Eip6466ReceiptSnapshot::build(Receipts::new(vec![Receipt::Basic(BasicReceipt {
                from_: Address::repeat_byte(2),
                gas_used: 1,
                logs: vec![log],
                status: true,
            })]))
            .unwrap();
        let whole =
            run(&snapshot, &single_request("[0].logs[0].data", SelectionOperation::Whole {}));
        assert_eq!(whole.results[0].values_ssz[0], Bytes::from(vec![0xab; size]));
        run(
            &snapshot,
            &single_request(
                "[0].logs[0].data",
                SelectionOperation::Slice { start: 0, end: size as u64 },
            ),
        );
        run(&snapshot, &single_request("[0].logs[0].topics", SelectionOperation::Whole {}));
    }
    let log = Log::new(Address::repeat_byte(3), vec![], Bytes::new()).unwrap();
    let receipt = Receipt::Basic(BasicReceipt {
        from_: Address::repeat_byte(2),
        gas_used: 1,
        logs: vec![log; 22],
        status: true,
    });
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(vec![receipt; 22])).unwrap();
    for index in [0, 1, 4, 5, 20, 21] {
        run(
            &snapshot,
            &single_request(
                &format!("[{index}].logs[{index}].address"),
                SelectionOperation::Value {},
            ),
        );
    }
}

#[test]
fn cancellation_and_noncanonical_chunks_fail_before_success() {
    let snapshot = fixture();
    assert_eq!(
        select_with_cancel(
            &snapshot,
            &single_request("[0].from", SelectionOperation::Value {}),
            SelectionLimits::default(),
            &AtomicBool::new(true)
        ),
        Err(SelectionError::Cancelled)
    );
    let target = Target { index: 1, kind: Kind::Data, length: Some(33) };
    assert_eq!(
        check_padding(target, &mut |_| Ok(B256::repeat_byte(1))),
        Err(SelectionError::InvalidValue)
    );
    assert_eq!(
        whole_root(Kind::Data, &[1; 33], 0),
        Err(SelectionError::LimitExceeded("source bytes"))
    );
    assert_eq!(
        whole_root(Kind::Topics, &[], 4 * size_of::<RetainedNode>()),
        Err(SelectionError::LimitExceeded("source bytes"))
    );
}

#[test]
fn small_object_range_is_not_charged_for_unrelated_receipts() {
    let basic = Receipt::Basic(BasicReceipt {
        from_: Address::ZERO,
        gas_used: 1,
        logs: vec![],
        status: true,
    });
    let mut receipts = vec![basic; 32];
    receipts.push(Receipt::Basic(BasicReceipt {
        from_: Address::ZERO,
        gas_used: 1,
        logs: vec![Log::new(Address::ZERO, vec![], Bytes::from(vec![7; 300_000])).unwrap()],
        status: true,
    }));
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(receipts)).unwrap();
    let mut request = single_request(".", SelectionOperation::Range { start: 0, end: 32 });
    let response = run(&snapshot, &request);
    assert_eq!(response.results[0].values_ssz.iter().map(|value| value.len()).sum::<usize>(), 1088);
    request.include_proof = false;
    let response =
        select_receipt_snapshot(&snapshot, &request, SelectionLimits::default()).unwrap();
    assert_eq!(response.results[0].values_ssz.len(), 32);
}

#[test]
fn whole_receipt_budget_follows_nested_shape_and_rejects_excess() {
    let snapshot =
        Eip6466ReceiptSnapshot::build(Receipts::new(vec![Receipt::Basic(BasicReceipt {
            from_: Address::ZERO,
            gas_used: 1,
            logs: vec![Log::new(Address::ZERO, vec![], Bytes::from(vec![7; 300_000])).unwrap()],
            status: true,
        })]))
        .unwrap();
    for path in [".", "[0]", "[0].logs", "[0].logs[0]", "[0].logs[0].data"] {
        let request = single_request(path, SelectionOperation::Whole {});
        let response = run(&snapshot, &request);
        assert_eq!(
            verify_receipt_selection(
                &request,
                &response,
                snapshot.root(),
                SelectionLimits { source_bytes: 4096, ..Default::default() }
            ),
            Err(SelectionError::LimitExceeded("source bytes"))
        );
    }
}

#[test]
fn disjoint_slices_and_ranges_share_a_path_without_overlapping_targets() {
    let snapshot = fixture();
    let mut request = SelectionRequest {
        selections: vec![
            ReceiptSelection {
                path: "[0].logs[0].data".into(),
                operation: SelectionOperation::Slice { start: 0, end: 2 },
            },
            ReceiptSelection {
                path: "[0].logs[0].data".into(),
                operation: SelectionOperation::Slice { start: 2, end: 4 },
            },
        ],
        include_proof: true,
    };
    let response = run(&snapshot, &request);
    assert_eq!(response.results[0].values_ssz, vec![Bytes::from(vec![0, 1])]);
    assert_eq!(response.results[1].values_ssz, vec![Bytes::from(vec![2, 3])]);
    request.selections[1].operation = SelectionOperation::Range { start: 2, end: 4 };
    run(&snapshot, &request);
    for operation in [
        SelectionOperation::Slice { start: 1, end: 4 },
        SelectionOperation::Slice { start: 0, end: 2 },
        SelectionOperation::Whole {},
        SelectionOperation::Length {},
    ] {
        request.selections[1].operation = operation;
        assert_eq!(
            select_receipt_snapshot(&snapshot, &request, SelectionLimits::default()),
            Err(SelectionError::DuplicateOrOverlap)
        );
    }
    request.selections[1] = ReceiptSelection {
        path: "[0].logs[0].data[1]".into(),
        operation: SelectionOperation::Value {},
    };
    assert_eq!(
        select_receipt_snapshot(&snapshot, &request, SelectionLimits::default()),
        Err(SelectionError::DuplicateOrOverlap)
    );
}

#[test]
fn whole_budget_rejects_malformed_offsets_before_materialization() {
    for bytes in
        [&[0_u8; 4][..], &[5, 0, 0, 0][..], &[8, 0, 0, 0, 4, 0, 0, 0][..], &[4, 0, 0, 0, 0xff][..]]
    {
        assert_eq!(
            check_whole_verification_budget(
                Kind::Receipts,
                bytes,
                SelectionLimits::default().source_bytes
            ),
            Err(SelectionError::InvalidValue)
        );
    }
    let snapshot = fixture();
    for receipt in snapshot.receipts().as_slice() {
        let bytes = receipt.to_ssz_bytes().unwrap();
        let mut required = bytes.len() * 8 + 4096;
        whole_structure_budget(
            Kind::Receipt(receipt.selector()),
            &bytes,
            &mut required,
            usize::MAX,
        )
        .unwrap();
        assert_eq!(
            check_whole_verification_budget(Kind::Receipt(receipt.selector()), &bytes, required),
            Ok(())
        );
        assert_eq!(
            check_whole_verification_budget(
                Kind::Receipt(receipt.selector()),
                &bytes,
                required - 1
            ),
            Err(SelectionError::LimitExceeded("source bytes"))
        );
    }
}

#[test]
fn structural_budget_covers_retained_trees_at_progressive_boundaries() {
    for count in [0, 1, 4, 5, 6, 20, 21, 22] {
        for length in [0, 1, 32, 33, 160, 161, 672, 673] {
            let log =
                Log::new(Address::ZERO, vec![B256::ZERO; 4], Bytes::from(vec![1; length])).unwrap();
            let receipt = Receipt::SetCode(SetCodeReceipt {
                from_: Address::ZERO,
                gas_used: 1,
                logs: vec![log; count],
                status: true,
                authorities: vec![Address::ZERO; count],
            });
            let snapshot =
                Eip6466ReceiptSnapshot::build(Receipts::new(vec![receipt; count])).unwrap();
            let mut bound = snapshot.serialized().len() * 8 + 4096;
            whole_structure_budget(Kind::Receipts, snapshot.serialized(), &mut bound, usize::MAX)
                .unwrap();
            let actual =
                source_budget(&snapshot, SelectionLimits::default(), &AtomicBool::new(false))
                    .unwrap();
            assert!(bound >= actual, "count={count} length={length} bound={bound} actual={actual}");
        }
    }
}
