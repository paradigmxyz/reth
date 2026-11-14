//! Checks the `balExecution` lines printed with `--bal-report`, the execution switch they report,
//! and what each runner does with a block's access list.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// The switch that selects the sequential executor.
const SEQUENTIAL: &str = "--bal-sequential";

/// A fixture with one empty block on genesis: a Paris one without an access list, or an
/// Amsterdam one (`*_amsterdam_*`) with one.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Runs the runner with `args` and returns its one result and the stderr lines that are
/// `balExecution` events.
fn run(args: &[&str]) -> (Value, Vec<Value>) {
    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner")).args(args).output().unwrap();
    let mut results: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), 1);
    let lines = String::from_utf8(output.stderr)
        .unwrap()
        .lines()
        .filter(|line| line.contains("balExecution"))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (results.remove(0), lines)
}

/// Runs the runner with `args`, checks that the fixture passed and returns the `path` and
/// `reason` of each `balExecution` line, all of which are for block 1.
fn decisions(args: &[&str]) -> Vec<(String, String)> {
    let (result, lines) = run(args);
    assert_eq!(result["pass"], true, "{}", result["error"]);
    lines
        .iter()
        .map(|line| {
            assert_eq!(line["block"], 1, "{line}");
            (
                line["path"].as_str().unwrap().to_string(),
                line["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn decision(path: &str, reason: &str) -> Vec<(String, String)> {
    vec![(path.to_string(), reason.to_string())]
}

#[test]
fn blocktest_reports_only_with_flag() {
    let fixture = fixture("blocktest_empty_block.json");
    let fixture = fixture.to_str().unwrap();

    assert_eq!(
        decisions(&["blocktest", fixture, "--bal-report"]),
        decision("sequential", "no-access-list")
    );
    assert!(decisions(&["blocktest", fixture]).is_empty());
}

#[test]
fn enginetest_reports_only_with_flag() {
    let fixture = fixture("enginetest_empty_block.json");
    let fixture = fixture.to_str().unwrap();
    let enginetest = ["enginetest", fixture, "--workers", "1"];

    assert_eq!(
        decisions(&[&enginetest[..], &["--bal-report"]].concat()),
        decision("sequential", "no-access-list")
    );
    assert!(decisions(&enginetest).is_empty());
}

/// The parallel executor runs a block with an access list unless the switch disables it.
#[test]
fn enginetest_runs_parallel_unless_disabled() {
    let fixture = fixture("enginetest_amsterdam_empty_block.json");
    let enginetest = ["enginetest", fixture.to_str().unwrap(), "--workers", "1", "--bal-report"];

    assert_eq!(decisions(&enginetest), decision("parallel", ""));
    assert_eq!(
        decisions(&[&enginetest[..], &[SEQUENTIAL]].concat()),
        decision("sequential", "disabled")
    );
}

/// A payload whose access list does not match its execution is INVALID on either executor.
#[test]
fn enginetest_rejects_a_bad_access_list_on_either_executor() {
    let fixture = fixture("enginetest_amsterdam_bad_access_list.json");
    let enginetest = ["enginetest", fixture.to_str().unwrap(), "--workers", "1"];

    for args in [&enginetest[..], &[&enginetest[..], &[SEQUENTIAL]].concat()] {
        let (result, _) = run(args);
        assert_eq!(result["pass"], true, "{args:?}: {}", result["error"]);
        assert_eq!(result["lastPayloadStatus"], "INVALID", "{args:?}");
    }
}

/// Block import attaches the delivered list when it hashes to the header's commitment, and drops
/// it otherwise, including when only the order of its entries differs. A dropped list does not
/// change the verdict, which follows the header.
#[test]
fn blocktest_drops_a_reordered_access_list() {
    let fixture = fixture("blocktest_amsterdam_empty_block.json");
    let fixture = fixture.to_str().unwrap();
    assert_eq!(
        decisions(&["blocktest", fixture, "--bal-report"]),
        decision("sequential", "block-import")
    );
    assert_eq!(
        decisions(&["blocktest", fixture, "--bal-report", SEQUENTIAL]),
        decision("sequential", "disabled")
    );

    let mut json: Value = serde_json::from_str(&fs::read_to_string(fixture).unwrap()).unwrap();
    let test = json.as_object_mut().unwrap().values_mut().next().unwrap();
    let access_list = test["blocks"][0]["blockAccessList"].as_array_mut().unwrap();
    assert!(access_list.len() > 1);
    access_list.reverse();
    let dir = tempfile::tempdir().unwrap();
    let reordered = dir.path().join("reordered.json");
    fs::write(&reordered, json.to_string()).unwrap();
    let reordered = reordered.to_str().unwrap();

    for mode in [&[][..], &[SEQUENTIAL]] {
        let args = [&["blocktest", reordered, "--bal-report"][..], mode].concat();
        assert_eq!(decisions(&args), decision("sequential", "bad-access-list"), "{mode:?}");
    }
}

/// With `--bal-withhold` a block executes without the list the fixture delivers, and its verdict
/// still follows the header.
#[test]
fn blocktest_withholds_the_access_list() {
    let amsterdam = fixture("blocktest_amsterdam_empty_block.json");
    for mode in [&[][..], &[SEQUENTIAL]] {
        let args = [
            &["blocktest", amsterdam.to_str().unwrap(), "--bal-report", "--bal-withhold"][..],
            mode,
        ]
        .concat();
        assert_eq!(decisions(&args), decision("sequential", "withheld"), "{mode:?}");
    }

    let paris = fixture("blocktest_empty_block.json");
    assert_eq!(
        decisions(&["blocktest", paris.to_str().unwrap(), "--bal-report", "--bal-withhold"]),
        decision("sequential", "no-access-list")
    );
}
