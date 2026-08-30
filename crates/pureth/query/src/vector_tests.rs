use crate::{
    vector::{decode_fixed, decode_hex},
    vector_records::{load_fixture, load_manifest, load_proof_case, load_root_case},
    verify_receipt_log_address, LogInput, ReceiptFixture, ReceiptInput,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

#[derive(Clone)]
struct Node {
    root: [u8; 32],
    children: Option<(Box<Self>, Box<Self>)>,
}

const fn leaf(root: [u8; 32]) -> Node {
    Node { root, children: None }
}

fn pair(left: Node, right: Node) -> Node {
    let mut hasher = Sha256::new();
    hasher.update(left.root);
    hasher.update(right.root);
    Node { root: hasher.finalize().into(), children: Some((Box::new(left), Box::new(right))) }
}

const fn zero() -> Node {
    leaf([0; 32])
}

fn uint_node(value: u64, byte_length: usize) -> Node {
    let mut root = [0; 32];
    root[..byte_length].copy_from_slice(&value.to_le_bytes()[..byte_length]);
    leaf(root)
}

fn bytes_node(value: &[u8]) -> Node {
    assert!(value.len() <= 32);
    let mut root = [0; 32];
    root[..value.len()].copy_from_slice(value);
    leaf(root)
}

fn merkleize(mut nodes: Vec<Node>, width: usize) -> Node {
    assert!(width.is_power_of_two() && nodes.len() <= width);
    nodes.resize_with(width, zero);
    while nodes.len() > 1 {
        nodes = nodes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|nodes| pair(nodes[0].clone(), nodes[1].clone()))
            .collect();
    }
    nodes.pop().unwrap()
}

fn mix_in_length(contents: Node, length: u64) -> Node {
    let mut length_node = [0; 32];
    length_node[..8].copy_from_slice(&length.to_le_bytes());
    pair(contents, leaf(length_node))
}

fn progressive_contents(nodes: &[Node], group_size: usize) -> Node {
    if nodes.is_empty() {
        return zero();
    }

    let split = nodes.len().min(group_size);
    pair(
        merkleize(nodes[..split].to_vec(), group_size),
        progressive_contents(&nodes[split..], group_size * 4),
    )
}

fn progressive_list(nodes: Vec<Node>, logical_length: usize) -> Node {
    mix_in_length(progressive_contents(&nodes, 1), logical_length as u64)
}

fn progressive_bytes(bytes: &[u8]) -> Node {
    progressive_list(bytes.chunks(32).map(bytes_node).collect(), bytes.len())
}

fn branch_at(root: &Node, gindex: u64) -> Vec<[u8; 32]> {
    let depth = u64::BITS - 1 - gindex.leading_zeros();
    let mut current = root;
    let mut branch = Vec::with_capacity(depth as usize);

    for shift in (0..depth).rev() {
        let (left, right) = current.children.as_ref().expect("path reached a leaf early");
        if (gindex >> shift) & 1 == 0 {
            branch.push(right.root);
            current = left;
        } else {
            branch.push(left.root);
            current = right;
        }
    }
    branch.reverse();
    branch
}

fn decode_node(value: &str) -> [u8; 32] {
    decode_fixed(value).unwrap()
}

fn build_log(log: &LogInput) -> Node {
    let topics = log.topics.iter().map(|topic| leaf(decode_node(topic))).collect::<Vec<_>>();
    merkleize(
        vec![
            bytes_node(&decode_fixed::<20>(&log.address).unwrap()),
            mix_in_length(merkleize(topics, 4), log.topics.len() as u64),
            progressive_bytes(&decode_hex(&log.data).unwrap()),
        ],
        4,
    )
}

fn build_receipt(receipt: &ReceiptInput) -> Node {
    let logs = receipt.logs.iter().map(build_log).collect::<Vec<_>>();
    let optional_address = match &receipt.contract_address {
        Some(address) => pair(bytes_node(&decode_fixed::<20>(address).unwrap()), uint_node(1, 1)),
        None => pair(zero(), uint_node(0, 1)),
    };
    merkleize(
        vec![
            uint_node(receipt.tx_type.into(), 1),
            uint_node(receipt.success.into(), 1),
            uint_node(receipt.gas_used, 8),
            optional_address,
            progressive_list(logs, receipt.logs.len()),
        ],
        8,
    )
}

fn build_receipts(fixture: &ReceiptFixture) -> Node {
    progressive_list(fixture.receipts.iter().map(build_receipt).collect(), fixture.receipts.len())
}

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data/fixtures/v0")
}

fn read_vector(path: &str) -> Vec<u8> {
    fs::read(vectors().join(path)).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::from("0x");
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[test]
fn manifest_binds_all_supplied_test_data() {
    let schema = include_bytes!("../test-data/pureth_receipt_v0.schema.json");
    let manifest = load_manifest(schema, &read_vector("manifest.json")).unwrap();
    let case_ids = manifest.cases.iter().map(|case| case.case_id.as_str()).collect::<Vec<_>>();
    assert_eq!(
        case_ids,
        [
            "empty_receipts",
            "singleton_baseline",
            "two_receipts",
            "two_logs",
            "ordering_mutation",
            "progressive_boundary",
            "value_mutation",
        ]
    );

    for case in &manifest.cases {
        assert!(!case.fixtures.is_empty());
        for fixture in &case.fixtures {
            assert_eq!(fixture.sha256, sha256_hex(&read_vector(&fixture.path)));
        }
    }
}

#[test]
fn every_proof_matches_the_independent_tree_and_verifier() {
    let schema = include_bytes!("../test-data/pureth_receipt_v0.schema.json");
    for (fixture_path, proof_path) in [
        ("singleton_baseline/fixture.json", "singleton_baseline/proof.json"),
        ("two_receipts/fixture.json", "two_receipts/proof.json"),
        ("two_logs/fixture.json", "two_logs/proof.json"),
        ("progressive_boundary/fixture_6.json", "progressive_boundary/proof_6.json"),
        ("value_mutation/fixture_mutated.json", "value_mutation/proof_mutated.json"),
    ] {
        let case =
            load_proof_case(schema, &read_vector(fixture_path), &read_vector(proof_path)).unwrap();
        let tree = build_receipts(&case.fixture.fixture);
        assert_eq!(tree.root, case.root, "{proof_path}");
        assert_eq!(branch_at(&tree, case.gindex), case.branch, "{proof_path}");
        assert_eq!(
            verify_receipt_log_address(
                &case.proof.schema_id,
                case.fixture.schema_digest,
                &case.proof.path,
                &case.fixture.log_counts,
                &case.selected_address,
                &case.branch,
                case.root,
            ),
            Ok(true),
            "{proof_path}"
        );
    }
}

#[test]
fn empty_ordering_boundary_and_value_mutation_records_match() {
    let schema = include_bytes!("../test-data/pureth_receipt_v0.schema.json");

    let empty = load_root_case(
        schema,
        &read_vector("empty_receipts/fixture.json"),
        &read_vector("empty_receipts/root.json"),
    )
    .unwrap();
    assert_eq!(empty.record.case_id, "empty_receipts");
    assert_eq!(build_receipts(&empty.fixture.fixture).root, empty.root);

    let ordering: Value =
        serde_json::from_slice(&read_vector("ordering_mutation/roots.json")).unwrap();
    let original =
        load_fixture(schema, &read_vector("ordering_mutation/fixture_original.json")).unwrap();
    let swapped =
        load_fixture(schema, &read_vector("ordering_mutation/fixture_swapped.json")).unwrap();
    let original_root = build_receipts(&original.fixture).root;
    let swapped_root = build_receipts(&swapped.fixture).root;
    assert_eq!(decode_node(ordering["original_root"].as_str().unwrap()), original_root);
    assert_eq!(decode_node(ordering["swapped_root"].as_str().unwrap()), swapped_root);
    assert_ne!(original_root, swapped_root);

    let boundary: Value =
        serde_json::from_slice(&read_vector("progressive_boundary/roots.json")).unwrap();
    let five = load_fixture(schema, &read_vector("progressive_boundary/fixture_5.json")).unwrap();
    let six = load_fixture(schema, &read_vector("progressive_boundary/fixture_6.json")).unwrap();
    let five_root = build_receipts(&five.fixture).root;
    let six_root = build_receipts(&six.fixture).root;
    assert_eq!(decode_node(boundary["five_root"].as_str().unwrap()), five_root);
    assert_eq!(decode_node(boundary["six_root"].as_str().unwrap()), six_root);
    assert_ne!(five_root, six_root);

    let original_proof = load_proof_case(
        schema,
        &read_vector("singleton_baseline/fixture.json"),
        &read_vector("singleton_baseline/proof.json"),
    )
    .unwrap();
    let mutated = load_proof_case(
        schema,
        &read_vector("value_mutation/fixture_mutated.json"),
        &read_vector("value_mutation/proof_mutated.json"),
    )
    .unwrap();
    assert_eq!(
        verify_receipt_log_address(
            &original_proof.proof.schema_id,
            original_proof.fixture.schema_digest,
            &original_proof.proof.path,
            &original_proof.fixture.log_counts,
            &mutated.selected_address,
            &original_proof.branch,
            original_proof.root,
        ),
        Ok(false)
    );
}

#[test]
fn loader_rejects_record_drift_before_proof_verification() {
    let schema = include_bytes!("../test-data/pureth_receipt_v0.schema.json");
    let fixture = read_vector("two_receipts/fixture.json");
    let proof = String::from_utf8(read_vector("two_receipts/proof.json"))
        .unwrap()
        .replace("\"log_counts\": [\n    1,\n    1\n  ]", "\"log_counts\": [1]");
    let error = load_proof_case(schema, &fixture, proof.as_bytes()).unwrap_err().to_string();
    assert!(error.contains("log counts mismatch"));
}
