//! Checks that a fixture fails when the client's head is not the fixture's `lastblockhash`,
//! including a fixture whose block the client rejects as expected.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// A hash that is no block of any fixture.
const WRONG_HASH: &str = "0x0202020202020202020202020202020202020202020202020202020202020202";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Writes the fixture `name` into `dir` with its `lastblockhash` replaced by [`WRONG_HASH`], and
/// returns the new file and the replaced hash.
fn with_wrong_last_block_hash(dir: &Path, name: &str) -> (PathBuf, String) {
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture(name)).unwrap()).unwrap();
    let test = json.as_object_mut().unwrap().values_mut().next().unwrap();
    let head = test["lastblockhash"].as_str().unwrap().to_string();
    test["lastblockhash"] = WRONG_HASH.into();
    let path = dir.join(name);
    fs::write(&path, json.to_string()).unwrap();
    (path, head)
}

/// Runs the runner with `args` on `name` as is, which passes, and with a wrong
/// `lastblockhash`, which fails with exit 1 and the error `expected_error(head)`, where `head` is
/// the fixture's real last block hash.
fn check(args: &[&str], name: &str, expected_error: fn(&str) -> String) {
    let run = |fixture: &Path| {
        let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner"))
            .args(&args[..1])
            .arg(fixture)
            .args(&args[1..])
            .output()
            .unwrap();
        let mut results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(results.len(), 1);
        (output.status.code(), results.remove(0))
    };

    let (code, result) = run(&fixture(name));
    assert_eq!(result["pass"], true, "{}", result["error"]);
    assert_eq!(code, Some(0));

    let dir = tempfile::tempdir().unwrap();
    let (wrong, head) = with_wrong_last_block_hash(dir.path(), name);
    let (code, result) = run(&wrong);
    assert_eq!(result["pass"], false);
    assert_eq!(code, Some(1));
    assert_eq!(result["error"], expected_error(&head));
    assert_eq!(result["lastBlockHash"], head);
}

fn blocktest_error(head: &str) -> String {
    format!("test failed: last block hash mismatch: got {head}, expected {WRONG_HASH}")
}

fn enginetest_error(head: &str) -> String {
    format!("test failed: last block hash mismatch: got Some({head}), expected {WRONG_HASH}")
}

#[test]
fn blocktest_wrong_last_block_hash() {
    check(&["blocktest"], "blocktest_empty_block.json", blocktest_error);
}

#[test]
fn enginetest_wrong_last_block_hash() {
    check(&["enginetest", "--workers", "1"], "enginetest_empty_block.json", enginetest_error);
}

/// The block is rejected as expected, so the head stays on genesis.
#[test]
fn blocktest_rejected_block_wrong_last_block_hash() {
    check(&["blocktest"], "blocktest_bad_state_root.json", blocktest_error);
}

#[test]
fn enginetest_rejected_payload_wrong_last_block_hash() {
    check(&["enginetest", "--workers", "1"], "enginetest_bad_state_root.json", enginetest_error);
}
