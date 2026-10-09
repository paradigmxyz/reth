//! End-to-end coverage of catalog registration and process-wide reporting.

use std::{cell::Cell, fs, process::Command};

use bedrock_assertions::Assertion;
use reth_dst::{always, catalog, init, sometimes, PropertyKind, Value};
use serde::{Serialize, Serializer};

#[test]
fn reporting_contract() {
    let executable = std::env::current_exe().unwrap();
    let path = std::env::temp_dir().join(format!("reth-dst-{}.jsonl", std::process::id()));
    for mode in ["enabled", "disabled", "failed"] {
        let mut child = Command::new(&executable);
        child.args(["--exact", "workload"]).env("RETH_DST_TEST_MODE", mode);
        match mode {
            "enabled" => {
                child.env("BEDROCK_ASSERTIONS_PATH", &path);
            }
            "failed" => {
                child.env("BEDROCK_ASSERTIONS_PATH", path.with_extension("missing").join("out"));
            }
            _ => {
                child.env_remove("BEDROCK_ASSERTIONS_PATH");
            }
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let records = fs::read_to_string(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let records: Vec<Value> =
        records.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(records.len(), 5);
    for record in &records {
        // The extended, timestamp-free record remains readable by Bedrock.
        let assertion: Assertion = serde_json::from_value(record.clone()).unwrap();
        assert_eq!(assertion.data().timestamp_unix_nano, 0);
        let data = record.as_object().unwrap().values().next().unwrap();
        assert_eq!(data["property_id"], data["message"]);
        assert_eq!(data.get("timestamp_unix_nano"), None);
    }
    let declarations: Vec<_> = records[..3]
        .iter()
        .map(|record| record.as_object().unwrap().values().next().unwrap())
        .collect();
    assert_eq!(
        declarations.iter().map(|data| data["message"].as_str().unwrap()).collect::<Vec<_>>(),
        ["never reached", "observed always", "observed sometimes"]
    );
    assert!(declarations.iter().all(|data| data["hit"] == false));
    assert_eq!(records[3]["Always"]["result"], false);
    assert_eq!(records[3]["Always"]["details"], serde_json::json!({"calls": 1}));
    assert_eq!(records[4]["Sometimes"]["result"], true);
}

#[test]
fn workload() {
    // This helper runs only in subprocesses launched by reporting_contract.
    // The ordinary test run intentionally skips it when no mode is configured.
    let Ok(mode) = std::env::var("RETH_DST_TEST_MODE") else { return };
    if mode == "failed" {
        let path = std::path::PathBuf::from(std::env::var_os("BEDROCK_ASSERTIONS_PATH").unwrap());
        let directory = path.parent().unwrap();
        let first = init().unwrap_err().to_string();
        fs::create_dir(directory).unwrap();
        assert_eq!(init().unwrap_err().to_string(), first);
        fs::remove_dir(directory).unwrap();
        return;
    }
    init().unwrap();
    let condition_calls = Cell::new(0);
    let details_calls = Cell::new(0);
    always!(
        {
            condition_calls.set(condition_calls.get() + 1);
            false
        },
        "observed always",
        {
            details_calls.set(details_calls.get() + 1);
            Details { disabled: mode == "disabled", calls: details_calls.get() }
        },
    );
    sometimes!(true, "observed sometimes");
    if false {
        always!(false, "never reached");
    }
    assert_eq!(condition_calls.get(), 1);
    assert_eq!(details_calls.get(), 1);
    let properties = catalog::properties();
    assert_eq!(properties.len(), 3);
    assert_eq!(properties[0].id(), "never reached");
    assert_eq!(properties[0].kind(), PropertyKind::Always);
    assert_eq!(properties[0].file(), file!());
    assert!(properties[0].line() > 0);
    assert!(properties[0].column() > 0);
    let mut exported = Vec::new();
    catalog::write(&mut exported).unwrap();
    assert_eq!(String::from_utf8(exported).unwrap().lines().count(), 3);
}

struct Details {
    disabled: bool,
    calls: u32,
}

impl Serialize for Details {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        assert!(!self.disabled, "disabled reporting must not serialize details");
        serde_json::json!({"calls": self.calls}).serialize(serializer)
    }
}
