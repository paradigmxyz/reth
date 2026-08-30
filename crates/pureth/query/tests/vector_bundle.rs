#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use reth_pureth_query::{load_manifest_proof_case, load_manifest_root_case};
use serde_json::Value;

const SCHEMA: &[u8] = include_bytes!("../test-data/pureth_receipt_v0.schema.json");
const MANIFEST: &[u8] = include_bytes!("../test-data/fixtures/v0/manifest.json");
const SINGLETON_FIXTURE: &[u8] =
    include_bytes!("../test-data/fixtures/v0/singleton_baseline/fixture.json");
const SINGLETON_PROOF: &[u8] =
    include_bytes!("../test-data/fixtures/v0/singleton_baseline/proof.json");
const TWO_RECEIPTS_PROOF: &[u8] =
    include_bytes!("../test-data/fixtures/v0/two_receipts/proof.json");
const EMPTY_FIXTURE: &[u8] = include_bytes!("../test-data/fixtures/v0/empty_receipts/fixture.json");
const EMPTY_ROOT: &[u8] = include_bytes!("../test-data/fixtures/v0/empty_receipts/root.json");

fn changed_json(bytes: &[u8], change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    change(&mut value);
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn loads_cases_only_when_manifest_fixture_and_record_are_bound() {
    let proof = load_manifest_proof_case(
        SCHEMA,
        MANIFEST,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        SINGLETON_PROOF,
    )
    .unwrap();
    assert_eq!(proof.proof.case_id, "singleton_baseline");

    let root = load_manifest_root_case(
        SCHEMA,
        MANIFEST,
        "empty_receipts",
        "empty_receipts/fixture.json",
        EMPTY_FIXTURE,
        EMPTY_ROOT,
    )
    .unwrap();
    assert_eq!(root.record.case_id, "empty_receipts");
}

#[test]
fn rejects_proof_and_root_case_id_drift() {
    let proof = changed_json(SINGLETON_PROOF, |value| {
        value["case_id"] = "two_receipts".into();
    });
    let error = load_manifest_proof_case(
        SCHEMA,
        MANIFEST,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        &proof,
    )
    .unwrap_err();
    assert!(error.to_string().contains("proof case ID"));

    let root = changed_json(EMPTY_ROOT, |value| {
        value["case_id"] = "singleton_baseline".into();
    });
    let error = load_manifest_root_case(
        SCHEMA,
        MANIFEST,
        "empty_receipts",
        "empty_receipts/fixture.json",
        EMPTY_FIXTURE,
        &root,
    )
    .unwrap_err();
    assert!(error.to_string().contains("root case ID"));
}

#[test]
fn rejects_files_or_hashes_from_another_case() {
    let error = load_manifest_proof_case(
        SCHEMA,
        MANIFEST,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        TWO_RECEIPTS_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("fixture digest"));

    let mut changed_fixture = SINGLETON_FIXTURE.to_vec();
    changed_fixture.push(b'\n');
    let error = load_manifest_proof_case(
        SCHEMA,
        MANIFEST,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        &changed_fixture,
        SINGLETON_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("manifest hash"));
}

#[test]
fn rejects_missing_duplicate_or_unsafe_manifest_cases() {
    let error = load_manifest_proof_case(
        SCHEMA,
        MANIFEST,
        "missing",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        SINGLETON_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not exist"));

    let duplicate = changed_json(MANIFEST, |value| {
        let case = value["cases"][1].clone();
        value["cases"].as_array_mut().unwrap().push(case);
    });
    let error = load_manifest_proof_case(
        SCHEMA,
        &duplicate,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        SINGLETON_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("duplicated"));

    let unsafe_path = changed_json(MANIFEST, |value| {
        value["cases"][0]["fixtures"][0]["path"] = "../fixture.json".into();
    });
    let error = load_manifest_proof_case(
        SCHEMA,
        &unsafe_path,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        SINGLETON_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("vector root"));

    let platform_path = changed_json(MANIFEST, |value| {
        value["cases"][0]["fixtures"][0]["path"] = "C:\\fixture.json".into();
    });
    let error = load_manifest_proof_case(
        SCHEMA,
        &platform_path,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        SINGLETON_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("vector root"));
}

#[test]
fn rejects_duplicates_outside_the_selected_case() {
    let duplicate_case = changed_json(MANIFEST, |value| {
        value["cases"][1]["case_id"] = value["cases"][0]["case_id"].clone();
    });
    let error = load_manifest_proof_case(
        SCHEMA,
        &duplicate_case,
        "two_receipts",
        "two_receipts/fixture.json",
        include_bytes!("../test-data/fixtures/v0/two_receipts/fixture.json"),
        TWO_RECEIPTS_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("case ID is duplicated"));

    let duplicate_fixture = changed_json(MANIFEST, |value| {
        let fixture = value["cases"][0]["fixtures"][0].clone();
        value["cases"][0]["fixtures"].as_array_mut().unwrap().push(fixture);
    });
    let error = load_manifest_proof_case(
        SCHEMA,
        &duplicate_fixture,
        "singleton_baseline",
        "singleton_baseline/fixture.json",
        SINGLETON_FIXTURE,
        SINGLETON_PROOF,
    )
    .unwrap_err();
    assert!(error.to_string().contains("fixture path is duplicated"));
}
