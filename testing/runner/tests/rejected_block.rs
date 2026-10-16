//! Checks the verdict on a block the client rejects: it depends on whether the fixture expects a
//! rejection, never on the reason it names, and the exit status agrees with it. The result reports
//! the rejection with reth's own error.

use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// A Paris fixture with one empty block on genesis whose state root is wrong, in the given
/// format, expecting `BlockException.INVALID_STATE_ROOT`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Runs the runner with `args` on `fixture`, checks that it exits 0 exactly when the fixture
/// passed, and returns its one result.
fn run(args: &[&str], fixture: &Path) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner"))
        .args(&args[..1])
        .arg(fixture)
        .args(&args[1..])
        .output()
        .unwrap();
    let mut results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.remove(0);
    let expected_code = if result["pass"] == true { 0 } else { 1 };
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    result
}

/// Writes `fixture` with `edit` applied to its one block or payload into `dir`.
fn edited(
    dir: &Path,
    name: &str,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> PathBuf {
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture(name)).unwrap()).unwrap();
    let test = json.as_object_mut().unwrap().values_mut().next().unwrap();
    let key = if test.get("blocks").is_some() { "blocks" } else { "engineNewPayloads" };
    let block = &mut test[key][0];
    edit(block.as_object_mut().unwrap());
    let path = dir.join(name);
    fs::write(&path, json.to_string()).unwrap();
    path
}

/// reth's error for the fixtures' block.
const STATE_ROOT_ERROR: &str = "mismatched block state root: got \
    0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421, expected \
    0x0101010101010101010101010101010101010101010101010101010101010101";

/// Runs the fixture as is, with its expected exception swapped for another, both of which pass,
/// and without an expected exception, which fails. Each reports the block's `rejection`.
fn check(args: &[&str], name: &str, exception_key: &str, rejection: serde_json::Value) {
    let result = run(args, &fixture(name));
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(result["rejections"], json!([rejection]));

    let dir = tempfile::tempdir().unwrap();
    let other_reason = edited(dir.path(), name, |block| {
        block.insert(
            exception_key.to_string(),
            "TransactionException.INSUFFICIENT_ACCOUNT_FUNDS".into(),
        );
    });
    let result = run(args, &other_reason);
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(result["rejections"], json!([rejection]));

    let expected_valid = edited(dir.path(), name, |block| {
        block.remove(exception_key);
    });
    let result = run(args, &expected_valid);
    assert_eq!(result["pass"], false);
    assert_eq!(result["rejections"], json!([rejection]));
}

#[test]
fn blocktest_rejected_block() {
    check(
        &["blocktest"],
        "blocktest_bad_state_root.json",
        "expectException",
        json!({
            "index": 0,
            "hash": "0x91883d9ff958b92d75733d3d8385e0b6fe583bc220dacc0b13046a932ed33847",
            "error": STATE_ROOT_ERROR,
        }),
    );
}

#[test]
fn enginetest_rejected_payload() {
    check(
        &["enginetest", "--workers", "1"],
        "enginetest_bad_state_root.json",
        "validationError",
        json!({"index": 0, "error": STATE_ROOT_ERROR}),
    );
}

/// A block that does not decode is reported with the decoder's error and no hash.
#[test]
fn blocktest_undecodable_block() {
    let dir = tempfile::tempdir().unwrap();
    let truncated = edited(dir.path(), "blocktest_bad_state_root.json", |block| {
        let rlp = block["rlp"].as_str().unwrap();
        let rlp = rlp.strip_suffix("c0").unwrap().to_string();
        block.insert("rlp".to_string(), rlp.into());
    });
    let result = run(&["blocktest"], &truncated);
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(result["rejections"], json!([{"index": 0, "error": "input too short"}]));
}

/// The blocks before one that does not decode are imported, so the head is the last of them.
#[test]
fn blocktest_undecodable_block_after_valid_block() {
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture("blocktest_empty_block.json")).unwrap())
            .unwrap();
    let test = json.as_object_mut().unwrap().values_mut().next().unwrap();
    test["blocks"].as_array_mut().unwrap().push(json!({
        "rlp": "0xf90200c0",
        "expectException": "BlockException.RLP_STRUCTURES_ENCODING",
    }));
    let last_block_hash = test["lastblockhash"].clone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blocktest_empty_block.json");
    fs::write(&path, json.to_string()).unwrap();

    let result = run(&["blocktest"], &path);
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(result["lastBlockHash"], last_block_hash);
    assert_eq!(result["rejections"], json!([{"index": 1, "error": "input too short"}]));
}

#[test]
fn clean_fixtures_report_no_rejections() {
    let result = run(&["blocktest"], &fixture("blocktest_empty_block.json"));
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(result["rejections"], json!([]));

    let result = run(&["enginetest", "--workers", "1"], &fixture("enginetest_empty_block.json"));
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(result["rejections"], json!([]));
}
