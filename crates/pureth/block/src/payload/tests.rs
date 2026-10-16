use super::*;
use crate::{ExecutionRequests, RequestsError};
use reth_pureth_receipt::eip6466::{BasicReceipt, Receipt, Receipts};
use serde_json::Value;

fn reference() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/payload.json")).unwrap()
}

fn bytes(value: &Value) -> Bytes {
    value.as_str().unwrap().parse().unwrap()
}

fn sidecars(case: &Value) -> (Eip6466ReceiptSnapshot, ExecutionRequestsSnapshot) {
    (
        Eip6466ReceiptSnapshot::build(Receipts::from_ssz_bytes(&bytes(&case["receipts"])).unwrap())
            .unwrap(),
        ExecutionRequestsSnapshot::from_ssz(&bytes(&case["requests"])).unwrap(),
    )
}

fn decoded(case: &Value) -> ExecutionPayloadSnapshot {
    let (receipts, requests) = sidecars(case);
    ExecutionPayloadSnapshot::from_ssz(
        &bytes(&case["payload"]),
        receipts,
        requests,
        PayloadLimits::default(),
    )
    .unwrap()
}

#[test]
fn payload_bytes_roots_and_summaries_match_independent_typed_references() {
    for case in reference()["cases"].as_array().unwrap() {
        let snapshot = decoded(case);
        assert_eq!(snapshot.serialized(), bytes(&case["payload"]).as_ref());
        assert_eq!(snapshot.root(), case["root"].as_str().unwrap().parse::<B256>().unwrap());
        assert_eq!(snapshot.summary().serialized(), bytes(&case["summary"]).as_ref());
        assert_eq!(
            snapshot.components().transactions.transactions().len(),
            case["count"].as_u64().unwrap() as usize
        );
        for (index, name) in
            [(0x2a, "transactions"), (0x167, "withdrawals"), (0x16b, "block_access_list")]
        {
            assert_eq!(
                snapshot.node(index).unwrap().root(),
                case["component_nodes"][name].as_str().unwrap().parse::<B256>().unwrap()
            );
        }
    }
}

#[test]
fn payload_composition_moves_original_component_trees_without_rebuilding() {
    let reference = reference();
    let case = &reference["cases"][1];
    let transactions = TransactionSnapshot::from_ssz(&bytes(&case["transactions"])).unwrap();
    let children = transactions.tree().children().unwrap().as_ptr();
    let (receipts, requests) = sidecars(case);
    let snapshot = ExecutionPayloadSnapshot::build(
        decoded(case).fields().clone(),
        PayloadComponents {
            transactions,
            receipts,
            requests,
            withdrawals: WithdrawalSnapshot::from_ssz(&bytes(&case["withdrawals"])).unwrap(),
            block_access_list: BlockAccessListSnapshot::from_rlp(Bytes::from_static(&[0xc0]), 1)
                .unwrap(),
        },
        PayloadLimits::default(),
    )
    .unwrap();
    assert_eq!(snapshot.components().transactions.tree().children().unwrap().as_ptr(), children);
    assert!(std::ptr::eq(snapshot.node(0x2a).unwrap(), snapshot.components().transactions.tree()));
    assert!(std::ptr::eq(
        snapshot.node(0x54).unwrap(),
        snapshot.components().transactions.tree().children().unwrap().as_ptr()
    ));
    assert!(snapshot.node(0).is_none());
    assert!(snapshot.node(u128::MAX).is_none());
}

#[test]
fn expanded_payload_nodes_produce_a_branch_against_the_block_root() {
    let snapshot = decoded(&reference()["cases"][1]);
    for index in [0x2a_u128, 0x54, 0x167, 0x2ce, 0x16b, 0x2d6, 0x2d7] {
        let mut node = snapshot.node(index).unwrap().root();
        let mut position = index;
        while position > 1 {
            let sibling = snapshot.node(position ^ 1).unwrap().root();
            node = if position & 1 == 0 {
                RetainedNode::pair(RetainedNode::leaf(node), RetainedNode::leaf(sibling)).root()
            } else {
                RetainedNode::pair(RetainedNode::leaf(sibling), RetainedNode::leaf(node)).root()
            };
            position >>= 1;
        }
        assert_eq!(node, snapshot.root());
    }
}

#[test]
fn payload_decoder_rejects_noncanonical_offsets_and_component_root_mismatches() {
    let reference = reference();
    let case = &reference["cases"][1];
    let original = bytes(&case["payload"]);
    for offset in [84, 168, 268, 352] {
        let mut corrupt = original.to_vec();
        corrupt[offset..offset + 4].copy_from_slice(&0_u32.to_le_bytes());
        let (receipts, requests) = sidecars(case);
        assert!(ExecutionPayloadSnapshot::from_ssz(
            &corrupt,
            receipts,
            requests,
            PayloadLimits::default()
        )
        .is_err());
    }
    for (offset, expected) in [(88, "receipts root"), (320, "requests root")] {
        let mut corrupt = original.to_vec();
        corrupt[offset] ^= 1;
        let (receipts, requests) = sidecars(case);
        assert!(
            matches!(ExecutionPayloadSnapshot::from_ssz(&corrupt, receipts, requests, PayloadLimits::default()), Err(PayloadError::ComponentMismatch(name)) if name == expected)
        );
    }
    for length in [0, 363, original.len() - 1] {
        let (receipts, requests) = sidecars(case);
        assert!(ExecutionPayloadSnapshot::from_ssz(
            &original[..length],
            receipts,
            requests,
            PayloadLimits::default()
        )
        .is_err());
    }
    let mut trailing = original.to_vec();
    trailing.push(0);
    let (receipts, requests) = sidecars(case);
    assert!(ExecutionPayloadSnapshot::from_ssz(
        &trailing,
        receipts,
        requests,
        PayloadLimits::default()
    )
    .is_err());
}

#[test]
fn payload_limits_and_transaction_receipt_counts_fail_closed() {
    let reference = reference();
    let case = &reference["cases"][1];
    for limits in [
        PayloadLimits { bytes: 363, ..PayloadLimits::default() },
        PayloadLimits { transactions: 0, ..PayloadLimits::default() },
        PayloadLimits { withdrawals: 0, ..PayloadLimits::default() },
        PayloadLimits { bal_bytes: 0, ..PayloadLimits::default() },
    ] {
        let (receipts, requests) = sidecars(case);
        assert!(matches!(
            ExecutionPayloadSnapshot::from_ssz(
                &bytes(&case["payload"]),
                receipts,
                requests,
                limits
            ),
            Err(PayloadError::LimitExceeded(_))
        ));
    }
    let snapshot = decoded(case);
    let fields = snapshot.fields().clone();
    let ExecutionPayloadSnapshot { mut components, .. } = snapshot;
    components.receipts = Eip6466ReceiptSnapshot::build(Receipts::new(Vec::new())).unwrap();
    assert!(matches!(
        ExecutionPayloadSnapshot::build(fields, components, PayloadLimits::default()),
        Err(PayloadError::ComponentMismatch("transaction/receipt count"))
    ));
}

#[test]
fn payload_exact_byte_limits_include_sidecars_and_reject_invalid_summary_fields() {
    let reference = reference();
    let case = &reference["cases"][1];
    let encoded = bytes(&case["payload"]);
    let (receipts, requests) = sidecars(case);
    let length = encoded.len() + receipts.serialized().len() + requests.serialized().len();
    let limits = PayloadLimits { bytes: length, ..PayloadLimits::default() };
    let snapshot =
        ExecutionPayloadSnapshot::from_ssz(&encoded, receipts, requests, limits).unwrap();
    assert_eq!(snapshot.serialized(), encoded.as_ref());
    let (receipts, requests) = sidecars(case);
    assert!(matches!(
        ExecutionPayloadSnapshot::from_ssz(
            &encoded,
            receipts,
            requests,
            PayloadLimits { bytes: length - 1, ..limits }
        ),
        Err(PayloadError::LimitExceeded("payload bytes"))
    ));
    for change in 0..3 {
        let ExecutionPayloadSnapshot { mut fields, components, .. } = decoded(case);
        match change {
            0 => fields.extra_data = Bytes::from(vec![0; 33]),
            1 => fields.gas_used.blob = fields.gas_limits.blob + 1,
            _ => fields.excess_gas.regular = 1,
        }
        assert!(matches!(
            ExecutionPayloadSnapshot::build(fields, components, PayloadLimits::default()),
            Err(PayloadError::Summary(_))
        ));
    }
}

#[test]
fn canonical_bal_bytes_are_distinct_from_empty_bytes_or_the_native_hash() {
    let snapshot = BlockAccessListSnapshot::from_rlp(Bytes::from_static(&[0xc0]), 1).unwrap();
    assert_eq!(snapshot.serialized(), &[0xc0]);
    assert_eq!(snapshot.native_hash(), keccak256([0xc0]));
    assert_ne!(snapshot.native_hash(), snapshot.root());
    for bytes in [&[][..], &[0x80], &[0xc0, 0], &[0xf8, 0], &[0xc1, 0x80]] {
        assert!(BlockAccessListSnapshot::from_rlp(Bytes::copy_from_slice(bytes), 16).is_err());
    }
}

#[test]
fn native_request_groups_preserve_records_and_reject_unknown_duplicate_and_empty_groups() {
    let reference = reference();
    let (_, expected) = sidecars(&reference["cases"][0]);
    let encoded = expected.serialized();
    let offsets = [
        u32::from_le_bytes(encoded[0..4].try_into().unwrap()) as usize,
        u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize,
        u32::from_le_bytes(encoded[8..12].try_into().unwrap()) as usize,
        encoded.len(),
    ];
    let groups: Vec<Bytes> = (0..3)
        .map(|index| {
            let mut bytes = vec![index as u8];
            bytes.extend_from_slice(&encoded[offsets[index]..offsets[index + 1]]);
            Bytes::from(bytes)
        })
        .collect();
    let snapshot = ExecutionRequestsSnapshot::from_native_groups(&groups).unwrap();
    assert_eq!(snapshot.serialized(), expected.serialized());
    assert_eq!(snapshot.root(), expected.root());
    for groups in [
        vec![groups[1].clone(), groups[0].clone()],
        vec![groups[0].clone(), groups[0].clone()],
        vec![Bytes::new()],
        vec![Bytes::from_static(&[0])],
        vec![Bytes::from_static(&[1, 0])],
    ] {
        assert!(ExecutionRequests::from_native_groups(&groups).is_err());
    }
    for tag in [3, 4, 255] {
        assert!(
            matches!(ExecutionRequests::from_native_groups(&[Bytes::from(vec![tag, 0])]), Err(RequestsError::UnsupportedType(actual)) if actual == tag)
        );
    }
    assert!(ExecutionRequests::from_native_groups(&[]).unwrap().deposits.is_empty());
    let mut oversized = vec![2];
    oversized.resize(1 + 116 * 3, 0);
    assert!(ExecutionRequests::from_native_groups(&[Bytes::from(oversized)]).is_err());
}

#[test]
fn required_sidecars_cannot_be_replaced_by_arbitrary_empty_snapshots() {
    let reference = reference();
    let case = &reference["cases"][1];
    let (_, requests) = sidecars(case);
    let receipts =
        Eip6466ReceiptSnapshot::build(Receipts::new(vec![Receipt::Basic(BasicReceipt {
            from_: Address::ZERO,
            gas_used: 1,
            logs: Vec::new(),
            status: false,
        })]))
        .unwrap();
    assert!(matches!(
        ExecutionPayloadSnapshot::from_ssz(
            &bytes(&case["payload"]),
            receipts,
            requests,
            PayloadLimits::default()
        ),
        Err(PayloadError::ComponentMismatch("receipts root"))
    ));
}
