//! RPC compatibility test actions for testing RPC methods against execution-apis test data.

use eyre::{eyre, Result};
use futures_util::future::BoxFuture;
use jsonrpsee::core::client::ClientT;
use reth_e2e_test_utils::testsuite::{actions::Action, BlockInfo, Environment};
use reth_node_api::EngineTypes;
use serde_json::Value;
use std::path::Path;
use tracing::{debug, info};

/// Test case from execution-apis .io file format
#[derive(Debug, Clone)]
pub struct RpcTestCase {
    /// The test name (filename without .io extension)
    pub name: String,
    /// Request to send (as JSON value)
    pub request: Value,
    /// Expected response (as JSON value)
    pub expected_response: Value,
    /// Whether this test is spec-only
    pub spec_only: bool,
}

/// Action that runs RPC compatibility tests from execution-apis test data
#[derive(Debug)]
pub struct RunRpcCompatTests {
    /// RPC methods to test (e.g., ["`eth_getLogs`"])
    pub methods: Vec<String>,
    /// Path to the execution-apis tests directory
    pub test_data_path: String,
    /// Whether to stop on first failure
    pub fail_fast: bool,
}

impl RunRpcCompatTests {
    /// Create a new RPC compatibility test runner
    pub fn new(methods: Vec<String>, test_data_path: impl Into<String>) -> Self {
        Self { methods, test_data_path: test_data_path.into(), fail_fast: false }
    }

    /// Set whether to stop on first failure
    pub const fn with_fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }

    /// Parse a .io test file
    fn parse_io_file(content: &str) -> Result<RpcTestCase> {
        let mut lines = content.lines();
        let mut spec_only = false;
        let mut request_line = None;
        let mut response_line = None;

        // Skip comments and look for spec_only marker
        for line in lines.by_ref() {
            let line = line.trim();
            if line.starts_with("//") {
                if line.contains("speconly:") {
                    spec_only = true;
                }
            } else if let Some(stripped) = line.strip_prefix(">>") {
                request_line = Some(stripped.trim());
                break;
            }
        }

        // Look for response
        for line in lines {
            let line = line.trim();
            if let Some(stripped) = line.strip_prefix("<<") {
                response_line = Some(stripped.trim());
                break;
            }
        }

        let request_str =
            request_line.ok_or_else(|| eyre!("No request found in test file (>> marker)"))?;
        let response_str =
            response_line.ok_or_else(|| eyre!("No response found in test file (<< marker)"))?;

        // Parse request
        let request: Value = serde_json::from_str(request_str)
            .map_err(|e| eyre!("Failed to parse request: {}", e))?;

        // Parse response
        let expected_response: Value = serde_json::from_str(response_str)
            .map_err(|e| eyre!("Failed to parse response: {}", e))?;

        Ok(RpcTestCase { name: String::new(), request, expected_response, spec_only })
    }

    /// Compare JSON values with special handling for numbers and errors
    /// Uses iterative approach to avoid stack overflow with deeply nested structures
    fn compare_json_values(actual: &Value, expected: &Value, path: &str) -> Result<()> {
        // Stack to hold work items: (actual, expected, path)
        let mut work_stack = vec![(actual, expected, path.to_string())];

        while let Some((actual, expected, current_path)) = work_stack.pop() {
            match (actual, expected) {
                // Number comparison: handle different representations
                (Value::Number(a), Value::Number(b)) => {
                    let a_f64 = a.as_f64().ok_or_else(|| eyre!("Invalid number"))?;
                    let b_f64 = b.as_f64().ok_or_else(|| eyre!("Invalid number"))?;
                    // Use a reasonable epsilon for floating point comparison
                    const EPSILON: f64 = 1e-10;
                    if (a_f64 - b_f64).abs() > EPSILON {
                        return Err(eyre!("Number mismatch at {}: {} != {}", current_path, a, b));
                    }
                }
                // Array comparison
                (Value::Array(a), Value::Array(b)) => {
                    if a.len() != b.len() {
                        return Err(eyre!(
                            "Array length mismatch at {}: {} != {}",
                            current_path,
                            a.len(),
                            b.len()
                        ));
                    }
                    // Add array elements to work stack in reverse order
                    // so they are processed in correct order
                    for (i, (av, bv)) in a.iter().zip(b.iter()).enumerate().rev() {
                        work_stack.push((av, bv, format!("{current_path}[{i}]")));
                    }
                }
                // Object comparison
                (Value::Object(a), Value::Object(b)) => {
                    // Check all keys in expected are present in actual
                    for (key, expected_val) in b {
                        if let Some(actual_val) = a.get(key) {
                            work_stack.push((
                                actual_val,
                                expected_val,
                                format!("{current_path}.{key}"),
                            ));
                        } else {
                            return Err(eyre!("Missing key at {}.{}", current_path, key));
                        }
                    }
                }
                // Direct value comparison
                (a, b) => {
                    if a != b {
                        return Err(eyre!("Value mismatch at {}: {:?} != {:?}", current_path, a, b));
                    }
                }
            }
        }
        Ok(())
    }

    /// Execute a single test case
    async fn execute_test_case<Engine: EngineTypes>(
        &self,
        test_case: &RpcTestCase,
        env: &Environment<Engine>,
    ) -> Result<()> {
        let node_client = &env.node_clients[env.active_node_idx];

        // Extract method and params from request
        let method = test_case
            .request
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre!("Request missing method field"))?;

        let params = test_case.request.get("params").cloned().unwrap_or(Value::Array(vec![]));

        // Make the RPC request using jsonrpsee
        // We need to handle the case where the RPC might return an error
        use jsonrpsee::core::params::ArrayParams;

        let response_result: Result<Value, jsonrpsee::core::client::Error> = match params {
            Value::Array(ref arr) => {
                // Use ArrayParams for array parameters
                let mut array_params = ArrayParams::new();
                for param in arr {
                    array_params
                        .insert(param.clone())
                        .map_err(|e| eyre!("Failed to insert param: {}", e))?;
                }
                node_client.rpc.request(method, array_params).await
            }
            _ => {
                // For non-array params, wrap in an array
                let mut array_params = ArrayParams::new();
                array_params.insert(params).map_err(|e| eyre!("Failed to insert param: {}", e))?;
                node_client.rpc.request(method, array_params).await
            }
        };

        // Build actual response object to match execution-apis format
        let actual_response = match response_result {
            Ok(response) => {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": test_case.request.get("id").cloned().unwrap_or(Value::Null),
                    "result": response
                })
            }
            Err(err) => {
                // RPC error - build error response
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": test_case.request.get("id").cloned().unwrap_or(Value::Null),
                    "error": {
                        "code": -32000, // Generic error code
                        "message": err.to_string()
                    }
                })
            }
        };

        // Compare responses
        let expected_result = test_case.expected_response.get("result");
        let expected_error = test_case.expected_response.get("error");
        let actual_result = actual_response.get("result");
        let actual_error = actual_response.get("error");

        match (expected_result, expected_error) {
            (Some(expected), None) => {
                // Expected success response
                if let Some(actual) = actual_result {
                    Self::compare_json_values(actual, expected, "result")?;
                } else if let Some(error) = actual_error {
                    return Err(eyre!("Expected success response but got error: {}", error));
                } else {
                    return Err(eyre!("Expected success response but got neither result nor error"));
                }
            }
            (None, Some(_)) => {
                // Expected error response - just check that we got an error
                if actual_error.is_none() {
                    return Err(eyre!("Expected error response but got success"));
                }
                debug!("Both responses are errors (expected behavior)");
            }
            _ => {
                return Err(eyre!("Invalid expected response format"));
            }
        }

        Ok(())
    }
}

impl<Engine> Action<Engine> for RunRpcCompatTests
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut total_tests = 0;
            let mut passed_tests = 0;

            for method in &self.methods {
                info!("Running RPC compatibility tests for {}", method);

                let method_dir = Path::new(&self.test_data_path).join(method);
                if !method_dir.exists() {
                    return Err(eyre!("Test directory does not exist: {}", method_dir.display()));
                }

                // Read all .io files in the method directory
                let entries = std::fs::read_dir(&method_dir)
                    .map_err(|e| eyre!("Failed to read directory: {}", e))?;

                for entry in entries {
                    let entry = entry?;
                    let path = entry.path();

                    if path.extension().and_then(|s| s.to_str()) == Some("io") {
                        let test_name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();

                        let content = std::fs::read_to_string(&path)
                            .map_err(|e| eyre!("Failed to read test file: {}", e))?;

                        match Self::parse_io_file(&content) {
                            Ok(mut test_case) => {
                                test_case.name = test_name.clone();
                                total_tests += 1;

                                match self.execute_test_case(&test_case, env).await {
                                    Ok(_) => {
                                        info!("✓ {}/{}: PASS", method, test_name);
                                        passed_tests += 1;
                                    }
                                    Err(e) => {
                                        info!("✗ {}/{}: FAIL - {}", method, test_name, e);

                                        if self.fail_fast {
                                            return Err(eyre!("Test failed (fail-fast enabled)"));
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                info!("✗ {}/{}: PARSE ERROR - {}", method, test_name, e);
                                if self.fail_fast {
                                    return Err(e);
                                }
                            }
                        }
                    }
                }
            }

            info!("RPC compatibility test results: {}/{} passed", passed_tests, total_tests);

            if passed_tests < total_tests {
                return Err(eyre!("Some tests failed: {}/{} passed", passed_tests, total_tests));
            }

            Ok(())
        })
    }
}

/// Action to initialize the chain from execution-apis test data
#[derive(Debug)]
pub struct InitializeFromExecutionApis {
    /// Path to the base.rlp file (if different from default)
    pub chain_rlp_path: Option<String>,
    /// Path to the headfcu.json file (if different from default)
    pub fcu_json_path: Option<String>,
}

impl Default for InitializeFromExecutionApis {
    fn default() -> Self {
        Self::new()
    }
}

impl InitializeFromExecutionApis {
    /// Create with default paths (assumes execution-apis/tests structure)
    pub const fn new() -> Self {
        Self { chain_rlp_path: None, fcu_json_path: None }
    }

    /// Set custom chain RLP path
    pub fn with_chain_rlp(mut self, path: impl Into<String>) -> Self {
        self.chain_rlp_path = Some(path.into());
        self
    }

    /// Set custom FCU JSON path
    pub fn with_fcu_json(mut self, path: impl Into<String>) -> Self {
        self.fcu_json_path = Some(path.into());
        self
    }
}

impl<Engine> Action<Engine> for InitializeFromExecutionApis
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Load forkchoice state
            let fcu_path = self
                .fcu_json_path
                .as_ref()
                .map(Path::new)
                .ok_or_else(|| eyre!("FCU JSON path is required"))?;

            let fcu_state = reth_e2e_test_utils::setup_import::load_forkchoice_state(fcu_path)?;

            info!(
                "Applying forkchoice state - head: {}, safe: {}, finalized: {}",
                fcu_state.head_block_hash,
                fcu_state.safe_block_hash,
                fcu_state.finalized_block_hash
            );

            // Apply forkchoice update to each node
            for (idx, client) in env.node_clients.iter().enumerate() {
                debug!("Applying forkchoice update to node {}", idx);

                // Wait for the node to finish syncing imported blocks
                let mut retries = 0;
                const MAX_RETRIES: u32 = 10;
                const RETRY_DELAY_MS: u64 = 500;

                loop {
                    let response =
                        reth_rpc_api::clients::EngineApiClient::<Engine>::fork_choice_updated_v3(
                            &client.engine.http_client(),
                            fcu_state,
                            None,
                        )
                        .await
                        .map_err(|e| eyre!("Failed to update forkchoice on node {}: {}", idx, e))?;

                    match response.payload_status.status {
                        alloy_rpc_types_engine::PayloadStatusEnum::Valid => {
                            debug!("Forkchoice update successful on node {}", idx);
                            break;
                        }
                        alloy_rpc_types_engine::PayloadStatusEnum::Syncing => {
                            if retries >= MAX_RETRIES {
                                return Err(eyre!(
                                    "Node {} still syncing after {} retries",
                                    idx,
                                    MAX_RETRIES
                                ));
                            }
                            debug!("Node {} is syncing, retrying in {}ms...", idx, RETRY_DELAY_MS);
                            tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS))
                                .await;
                            retries += 1;
                        }
                        _ => {
                            return Err(eyre!(
                                "Invalid forkchoice state on node {}: {:?}",
                                idx,
                                response.payload_status
                            ));
                        }
                    }
                }
            }

            // Update environment state
            env.active_node_state_mut()?.current_block_info = Some(BlockInfo {
                hash: fcu_state.head_block_hash,
                number: 0, // Will be updated when we fetch the actual block
                timestamp: 0,
            });

            info!("Successfully initialized chain from execution-apis test data");
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compare_json_values_deeply_nested() {
        // Test that the iterative comparison handles deeply nested structures
        // without stack overflow
        let mut nested = json!({"value": 0});
        let mut expected = json!({"value": 0});

        // Create a deeply nested structure
        for i in 1..1000 {
            nested = json!({"level": i, "nested": nested});
            expected = json!({"level": i, "nested": expected});
        }

        // Should not panic with stack overflow
        RunRpcCompatTests::compare_json_values(&nested, &expected, "root").unwrap();
    }

    #[test]
    fn test_compare_json_values_arrays() {
        // Test array comparison
        let actual = json!([1, 2, 3, 4, 5]);
        let expected = json!([1, 2, 3, 4, 5]);

        RunRpcCompatTests::compare_json_values(&actual, &expected, "root").unwrap();

        // Test array length mismatch
        let actual = json!([1, 2, 3]);
        let expected = json!([1, 2, 3, 4, 5]);

        let result = RunRpcCompatTests::compare_json_values(&actual, &expected, "root");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Array length mismatch"));
    }

    #[test]
    fn test_compare_json_values_objects() {
        // Test object comparison
        let actual = json!({"a": 1, "b": 2, "c": 3});
        let expected = json!({"a": 1, "b": 2, "c": 3});

        RunRpcCompatTests::compare_json_values(&actual, &expected, "root").unwrap();

        // Test missing key
        let actual = json!({"a": 1, "b": 2});
        let expected = json!({"a": 1, "b": 2, "c": 3});

        let result = RunRpcCompatTests::compare_json_values(&actual, &expected, "root");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing key"));
    }

    #[test]
    fn test_compare_json_values_numbers() {
        // Test number comparison with floating point
        let actual = json!({"value": 1.00000000001});
        let expected = json!({"value": 1.0});

        // Should be equal within epsilon (1e-10)
        RunRpcCompatTests::compare_json_values(&actual, &expected, "root").unwrap();

        // Test significant difference
        let actual = json!({"value": 1.1});
        let expected = json!({"value": 1.0});

        let result = RunRpcCompatTests::compare_json_values(&actual, &expected, "root");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Number mismatch"));
    }
}
