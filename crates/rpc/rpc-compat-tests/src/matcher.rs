//! Hive-compatible JSON response matching.

use crate::schema::SchemaCatalog;
use eyre::{eyre, Result};
use serde_json::Value;
use std::collections::BTreeSet;

/// Matches one actual response against acceptable expected responses.
pub fn compare(
    actual: &Value,
    expected: &[Value],
    spec_only: bool,
    method: &str,
    schemas: &SchemaCatalog,
) -> Result<()> {
    if spec_only && actual.get("error").is_none() {
        let result =
            actual.get("result").ok_or_else(|| eyre!("successful response has no result field"))?;
        return schemas.validate(method, result)
    }

    let mut failures = Vec::new();
    for (index, candidate) in expected.iter().enumerate() {
        let mut actual = actual.clone();
        let mut candidate = candidate.clone();
        redact_error_messages(&mut actual, &mut candidate);
        let mut differences = Vec::new();
        collect_differences(&actual, &candidate, "$", &mut differences);
        if differences.is_empty() {
            return Ok(())
        }
        failures.push(format!(
            "expected response {}:\n{}",
            index + 1,
            differences
                .into_iter()
                .map(|difference| format!("  - {difference}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Err(eyre!("response differs from every accepted response:\n{}", failures.join("\n")))
}

fn collect_differences(
    actual: &Value,
    expected: &Value,
    path: &str,
    differences: &mut Vec<String>,
) {
    match (actual, expected) {
        (Value::Number(left), Value::Number(right)) => {
            let equal = match (left.as_f64(), right.as_f64()) {
                (Some(left), Some(right)) => left == right || left.is_nan() && right.is_nan(),
                _ => left == right,
            };
            if !equal {
                differences.push(format!("{path}: expected {right}, received {left}"));
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                differences.push(format!(
                    "{path}: expected array length {}, received {}",
                    right.len(),
                    left.len()
                ));
            }
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                collect_differences(left, right, &format!("{path}[{index}]"), differences);
            }
            for (index, value) in right.iter().enumerate().skip(left.len()) {
                differences.push(format!("{path}[{index}]: missing expected value {value}"));
            }
            for (index, value) in left.iter().enumerate().skip(right.len()) {
                differences.push(format!("{path}[{index}]: unexpected value {value}"));
            }
        }
        (Value::Object(left), Value::Object(right)) => {
            let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                let child_path = format!("{path}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(actual), Some(expected)) => {
                        collect_differences(actual, expected, &child_path, differences);
                    }
                    (None, Some(expected)) => {
                        differences.push(format!("{child_path}: missing expected value {expected}"))
                    }
                    (Some(actual), None) => {
                        differences.push(format!("{child_path}: unexpected value {actual}"));
                    }
                    (None, None) => unreachable!(),
                }
            }
        }
        _ if actual == expected => {}
        _ => differences.push(format!("{path}: expected {expected}, received {actual}")),
    }
}

fn redact_error_messages(actual: &mut Value, expected: &mut Value) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for (key, expected_child) in expected.iter_mut() {
                let Some(actual_child) = actual.get_mut(key) else { continue };
                if key == "error" &&
                    actual_child.get("message").is_some() &&
                    expected_child.get("message").is_some()
                {
                    actual_child.as_object_mut().unwrap().remove("message");
                    expected_child.as_object_mut().unwrap().remove("message");
                } else {
                    redact_error_messages(actual_child, expected_child);
                }
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            for (actual, expected) in actual.iter_mut().zip(expected) {
                redact_error_messages(actual, expected);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_every_structural_difference() {
        let actual = json!({"result": {"a": 2, "extra": true, "items": [1, 3]}});
        let expected = json!({"result": {"a": 1, "items": [1, 2, 4], "missing": false}});
        let error = compare(&actual, &[expected], false, "eth_test", &SchemaCatalog::default())
            .unwrap_err()
            .to_string();

        for path in [
            "$.result.a",
            "$.result.extra",
            "$.result.items",
            "$.result.items[1]",
            "$.result.items[2]",
            "$.result.missing",
        ] {
            assert!(error.contains(path), "missing {path} in:\n{error}");
        }
    }

    #[test]
    fn ignores_only_corresponding_error_messages() {
        let actual =
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "reth"}});
        let expected =
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "fixture"}});
        assert!(compare(&actual, &[expected], false, "eth_call", &SchemaCatalog::default()).is_ok());
    }
}
