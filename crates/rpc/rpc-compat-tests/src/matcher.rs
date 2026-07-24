//! Hive-compatible JSON response matching.

use crate::schema::SchemaCatalog;
use eyre::{eyre, Result};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

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
        return schemas.validate(method, result);
    }

    let mut failures = Vec::new();
    for (index, candidate) in expected.iter().enumerate() {
        let mut actual = actual.clone();
        let mut candidate = candidate.clone();
        let redacted = redact_error_messages(&mut actual, &mut candidate);
        if equivalent(&actual, &candidate) {
            return Ok(());
        }
        let mut failure = String::new();
        if expected.len() > 1 {
            failure.push_str(&format!("expected response {}:\n", index + 1));
        }
        if redacted {
            failure.push_str("note: error messages removed from comparison\n");
        }
        failure.push_str("response differs from expected (-- client, ++ test):\n");
        failure.push_str(&json_diff(&actual, &candidate)?);
        failures.push(failure);
    }
    Err(eyre!("{}", failures.join("\n")))
}

fn equivalent(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(left), Value::Number(right)) => match (left.as_f64(), right.as_f64()) {
            (Some(left), Some(right)) => left == right || left.is_nan() && right.is_nan(),
            _ => left == right,
        },
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() &&
                left.iter().zip(right).all(|(left, right)| equivalent(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len() &&
                right.iter().all(|(key, expected)| {
                    left.get(key).is_some_and(|actual| equivalent(actual, expected))
                })
        }
        _ => actual == expected,
    }
}

fn json_diff(actual: &Value, expected: &Value) -> Result<String> {
    let actual = serde_json::to_string_pretty(actual)?;
    let expected = serde_json::to_string_pretty(expected)?;
    let actual_lines = actual.lines().collect::<Vec<_>>();
    let expected_lines = expected.lines().collect::<Vec<_>>();
    // Ignore commas while aligning lines so removing an object field does not also report the
    // preceding field merely because its trailing-comma status changed.
    let actual_comparison =
        actual_lines.iter().map(|line| line.strip_suffix(',').unwrap_or(line)).collect::<Vec<_>>();
    let expected_comparison = expected_lines
        .iter()
        .map(|line| line.strip_suffix(',').unwrap_or(line))
        .collect::<Vec<_>>();
    let diff = TextDiff::from_slices(&actual_comparison, &expected_comparison);
    let mut output = String::new();
    for change in diff.iter_all_changes() {
        let line = match change.tag() {
            ChangeTag::Insert => expected_lines[change.new_index().unwrap()],
            ChangeTag::Delete | ChangeTag::Equal => actual_lines[change.old_index().unwrap()],
        };
        let indentation = line.len() - line.trim_start_matches(' ').len();
        let marker = match change.tag() {
            ChangeTag::Delete => "-- ",
            ChangeTag::Insert => "++ ",
            ChangeTag::Equal => "",
        };
        output.push_str(&" ".repeat(indentation));
        output.push_str(marker);
        output.push_str(&line[indentation..]);
        output.push('\n');
    }
    Ok(output)
}

fn redact_error_messages(actual: &mut Value, expected: &mut Value) -> bool {
    let mut redacted = false;
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
                    redacted = true;
                } else {
                    redacted |= redact_error_messages(actual_child, expected_child);
                }
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            for (actual, expected) in actual.iter_mut().zip(expected) {
                redacted |= redact_error_messages(actual, expected);
            }
        }
        _ => {}
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_clean_json_diff() {
        let actual = json!({"result": {"a": 2, "extra": true, "items": [1, 3]}});
        let expected = json!({"result": {"a": 1, "items": [1, 2, 4], "missing": false}});
        let error = compare(&actual, &[expected], false, "eth_test", &SchemaCatalog::default())
            .unwrap_err()
            .to_string();

        for difference in [
            "-- \"a\": 2,",
            "++ \"a\": 1,",
            "-- \"extra\": true,",
            "-- 3",
            "++ 2,",
            "++ 4",
            "++ \"missing\": false",
        ] {
            assert!(error.contains(difference), "missing {difference} in:\n{error}");
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

    #[test]
    fn notes_redacted_error_messages() {
        let actual = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32602, "message": "reth", "data": "client data"}
        });
        let expected = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32602, "message": "fixture"}
        });
        let error = compare(&actual, &[expected], false, "eth_call", &SchemaCatalog::default())
            .unwrap_err()
            .to_string();

        assert!(error.contains("note: error messages removed from comparison"));
        assert!(error.contains("    \"code\": -32602,\n    -- \"data\": \"client data\""));
        assert!(!error.contains("-- \"code\""));
        assert!(!error.contains("++ \"code\""));
        assert!(!error.contains("\"message\""));
    }
}
