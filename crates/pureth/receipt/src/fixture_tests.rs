use super::*;
use serde_json::{json, Value};
use std::{path::Path, str::FromStr};

fn read_json(path: &str) -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../query/test-data/fixtures/v0").join(path);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn hex<T: FromStr>(value: &Value) -> Option<T> {
    let text = value.as_str()?;
    if !text.starts_with("0x") || text.len() % 2 != 0 {
        return None;
    }
    text.parse().ok()
}

fn parse_fixture(fixture: &Value) -> Option<ReceiptsSsz> {
    if fixture["schema_id"] != "pureth-receipt-v0" || fixture["root_type"] != "ReceiptsSSZ" {
        return None;
    }
    let receipts = fixture["receipts"].as_array()?;
    if fixture["receipt_count"].as_u64()? != u64::try_from(receipts.len()).ok()? {
        return None;
    }
    let receipts = receipts
        .iter()
        .map(|receipt| {
            let contract_address = match receipt.get("contract_address")? {
                Value::Null => None,
                value => Some(hex(value)?),
            };
            let logs = receipt["logs"]
                .as_array()?
                .iter()
                .map(|log| {
                    let topics = log["topics"].as_array()?;
                    if topics.len() > MAX_TOPICS {
                        return None;
                    }
                    Some(LogSsz {
                        address: hex(&log["address"])?,
                        topics: topics.iter().map(hex).collect::<Option<Vec<_>>>()?,
                        data: hex(&log["data"])?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(ReceiptSsz {
                tx_type: u8::try_from(receipt["tx_type"].as_u64()?).ok()?,
                success: receipt["success"].as_bool()?,
                gas_used: receipt["gas_used"].as_u64()?,
                contract_address,
                logs,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ReceiptsSsz(receipts))
}

fn snapshot(fixture: &Value) -> ReceiptSnapshot {
    ReceiptSnapshot::build(parse_fixture(fixture).expect("valid V0 fixture")).unwrap()
}

#[test]
fn all_frozen_roots_match_production_snapshots() {
    for (fixture, record, key) in [
        ("empty_receipts/fixture.json", "empty_receipts/root.json", "root"),
        ("singleton_baseline/fixture.json", "singleton_baseline/proof.json", "root"),
        ("two_receipts/fixture.json", "two_receipts/proof.json", "root"),
        ("two_logs/fixture.json", "two_logs/proof.json", "root"),
        (
            "ordering_mutation/fixture_original.json",
            "ordering_mutation/roots.json",
            "original_root",
        ),
        ("ordering_mutation/fixture_swapped.json", "ordering_mutation/roots.json", "swapped_root"),
        ("progressive_boundary/fixture_5.json", "progressive_boundary/roots.json", "five_root"),
        ("progressive_boundary/fixture_6.json", "progressive_boundary/roots.json", "six_root"),
        ("value_mutation/fixture_original.json", "value_mutation/result.json", "original_root"),
        ("value_mutation/fixture_mutated.json", "value_mutation/result.json", "mutated_root"),
    ] {
        let fixture_value = read_json(fixture);
        let snapshot = snapshot(&fixture_value);
        assert_eq!(snapshot.root(), hex::<B256>(&read_json(record)[key]).unwrap(), "{fixture}");
        assert_eq!(snapshot.root(), snapshot.tree().root());
        assert_eq!(snapshot.receipts(), &parse_fixture(&fixture_value).unwrap());
    }
}

#[test]
fn frozen_targets_are_retained_in_the_same_snapshot() {
    for (fixture, proof) in [
        ("singleton_baseline/fixture.json", "singleton_baseline/proof.json"),
        ("two_receipts/fixture.json", "two_receipts/proof.json"),
        ("two_logs/fixture.json", "two_logs/proof.json"),
        ("progressive_boundary/fixture_6.json", "progressive_boundary/proof_6.json"),
        ("value_mutation/fixture_mutated.json", "value_mutation/proof_mutated.json"),
    ] {
        let fixture = read_json(fixture);
        let proof = read_json(proof);
        let snapshot = snapshot(&fixture);
        let gindex = proof["gindex"].as_str().unwrap().parse::<u64>().unwrap();
        assert!(gindex > 0);
        let siblings = proof["proof"].as_array().unwrap();
        assert_eq!(siblings.len(), usize::try_from(gindex.ilog2()).unwrap());
        let mut node = snapshot.tree();
        for bit in (0..gindex.ilog2()).rev() {
            let children = node.children().expect("target path retains children");
            let direction = usize::from((gindex >> bit) & 1 != 0);
            assert_eq!(
                children[1 - direction].root(),
                hex::<B256>(&siblings[usize::try_from(bit).unwrap()]).unwrap()
            );
            node = &children[direction];
        }
        assert_eq!(node.root(), hex::<B256>(&proof["target_node"]).unwrap());
        assert_eq!(snapshot.root(), hex::<B256>(&proof["root"]).unwrap());
        let receipt_index =
            usize::try_from(fixture["first_target"]["receipt_index"].as_u64().unwrap()).unwrap();
        let log_index =
            usize::try_from(fixture["first_target"]["log_index"].as_u64().unwrap()).unwrap();
        let address = snapshot.receipts().get(receipt_index).unwrap().logs()[log_index].address();
        assert_eq!(address, hex::<Address>(&proof["value_ssz"]).unwrap());
        let mut chunk = [0_u8; 32];
        chunk[..20].copy_from_slice(address.as_slice());
        assert_eq!(node.root(), B256::from(chunk));
        assert_eq!(
            u64::try_from(snapshot.receipts().len()).unwrap(),
            proof["receipt_count"].as_u64().unwrap()
        );
        let counts = proof["log_counts"].as_array().unwrap();
        assert_eq!(snapshot.receipts().len(), counts.len());
        for (receipt, count) in snapshot.receipts().0.iter().zip(counts) {
            assert_eq!(u64::try_from(receipt.logs().len()).unwrap(), count.as_u64().unwrap());
        }
    }
}

#[test]
fn fixture_adapter_rejects_invalid_values() {
    let baseline = read_json("singleton_baseline/fixture.json");
    for (path, value) in [
        ("/schema_id", json!("wrong")),
        ("/root_type", json!("wrong")),
        ("/receipt_count", json!(2)),
        ("/receipts/0/tx_type", json!(256)),
        ("/receipts/0/success", json!(1)),
        ("/receipts/0/gas_used", json!(-1)),
        ("/receipts/0/contract_address", json!("0x00")),
        ("/receipts/0/logs/0/address", json!("0x00")),
        ("/receipts/0/logs/0/topics", json!(vec!["0x00"; 5])),
        ("/receipts/0/logs/0/topics/0", json!("0x00")),
        ("/receipts/0/logs/0/data", json!("0x0")),
        ("/receipts/0/logs/0/data", json!("0xgg")),
    ] {
        let mut fixture = baseline.clone();
        *fixture.pointer_mut(path).unwrap() = value;
        assert!(parse_fixture(&fixture).is_none(), "{path}");
    }
    let mut fixture = baseline;
    fixture["receipts"][0].as_object_mut().unwrap().remove("contract_address");
    assert!(parse_fixture(&fixture).is_none());
}

#[test]
fn data_boundaries_match_independent_roots() {
    for (length, expected) in [
        (0, "e95dc91dfe0ead18773ba74bf9ca93ff73faf2edc975e3b16e46d18a318fb7e7"),
        (1, "9f9acb7c6d18c863e1771db012fcc7460680e92c775f4478609b8d01112d51fb"),
        (31, "944b4d3c3687396dd67d71f18ce75d6b1daca1af5fd1c79910f31764d7a778d5"),
        (32, "eb14965de3bc2598c42e9c80be7e1cec8e62c74316726400c19c5957f71f49bb"),
        (33, "75cec4ae3333fcc590a06ae7e8a9e06d3c297d382216c1adda0f128c2b0728f6"),
        (160, "b12847470e662f7112f3d4927e7fc000ed77a79447b810b1db780537a1f04853"),
        (161, "33bb562f2d8a011cf17e030cfffaeaec4011dff65505252060e629c2b5131b56"),
    ] {
        let mut fixture = read_json("singleton_baseline/fixture.json");
        fixture["receipts"][0]["logs"][0]["data"] = json!(format!("0x{}", "ab".repeat(length)));
        assert_eq!(snapshot(&fixture).root(), expected.parse::<B256>().unwrap(), "{length}");
    }
}

#[test]
fn optional_address_and_topic_limits_match_independent_roots() {
    let baseline = read_json("singleton_baseline/fixture.json");
    let mut fixture = baseline.clone();
    fixture["receipts"][0]["contract_address"] = json!(format!("0x{}", "00".repeat(20)));
    assert_eq!(
        snapshot(&fixture).root(),
        alloy_primitives::b256!("140cb7cfd41ca3997fc12846e499629ebf6f1abd14cbb4b02832bacb033cfddf")
    );
    assert_ne!(snapshot(&baseline).root(), snapshot(&fixture).root());
    for (count, expected) in [
        (0_u8, "783c38ee00111092341712dbe1e69a1769781bf4f7618bd110a36950c986ba95"),
        (4, "2403f6a30bdc77fe3fc1f8adfdcc9a313feaf20f3d9c7a1571fc16e64cb3552c"),
    ] {
        let mut fixture = baseline.clone();
        fixture["receipts"][0]["logs"][0]["topics"] = json!((0..count)
            .map(|i| format!("0x{}", format!("{:02x}", 0x22 + i).repeat(32)))
            .collect::<Vec<_>>());
        assert_eq!(snapshot(&fixture).root(), expected.parse::<B256>().unwrap());
    }
}

#[test]
fn topic_mutations_and_trailing_zero_change_snapshot_root() {
    let baseline = read_json("singleton_baseline/fixture.json");
    let original = snapshot(&baseline).root();
    let mut fixture = baseline.clone();
    fixture["receipts"][0]["logs"][0]["topics"][0] = json!(format!("0x{}", "33".repeat(32)));
    assert_ne!(original, snapshot(&fixture).root());
    let mut fixture = baseline.clone();
    fixture["receipts"][0]["logs"][0]["data"] = json!("0x01020300");
    assert_ne!(original, snapshot(&fixture).root());
    let mut fixture = baseline;
    let topics = vec![format!("0x{}", "22".repeat(32)), format!("0x{}", "33".repeat(32))];
    fixture["receipts"][0]["logs"][0]["topics"] = json!(topics);
    let original = snapshot(&fixture).root();
    fixture["receipts"][0]["logs"][0]["topics"].as_array_mut().unwrap().swap(0, 1);
    assert_ne!(original, snapshot(&fixture).root());
}
