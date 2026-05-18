//! ExternEVM custom EvmFactory.
//!
//! Wraps the standard `EthEvmFactory` and injects the API_CALL precompile
//! at address 0x00000000000000000000000000000000000000AA.
//!
//! Milestone 1: any input → uint256(1234)
//! Milestone 3: decode ApiRequest, validate, return mock responses
//! Milestone 4: real HTTP calls via reqwest::blocking

use alloy_evm::{
    eth::EthEvmContext,
    evm::EvmFactory,
    precompiles::{DynPrecompile, PrecompileInput, PrecompilesMap},
    Database, Evm, EvmEnv,
};
use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{SolValue, sol};
use revm::{
    context::{BlockEnv, TxEnv},
    context_interface::result::{EVMError, HaltReason},
    inspector::NoOpInspector,
    precompile::{PrecompileHalt, PrecompileId, PrecompileOutput, PrecompileResult},
    primitives::hardfork::SpecId,
    Inspector,
};
use serde_json::Value as JsonValue;
use std::time::Duration;

/// The address of the API_CALL precompile: 0x00000000000000000000000000000000000000AA
pub const API_CALL_ADDRESS: Address = {
    let mut addr = [0u8; 20];
    addr[19] = 0xAA;
    Address::new(addr)
};

/// Fixed gas cost for API_CALL precompile.
const API_CALL_GAS: u64 = 3_000;

/// Maximum request body size in bytes.
const MAX_BODY_SIZE: usize = 4096;

/// Maximum response body size in bytes.
const MAX_RESPONSE_SIZE: usize = 8192;

/// HTTP timeout in milliseconds.
const HTTP_TIMEOUT_MS: u64 = 5000;

// ---------------------------------------------------------------------------
// ABI struct definition — matches the Solidity struct exactly.
// ---------------------------------------------------------------------------
sol! {
    #[derive(Debug)]
    struct ApiRequest {
        string url;
        string method;
        bytes headers;
        bytes body;
        string responsePath;
        uint8 responseType;
    }
}

// ---------------------------------------------------------------------------
// URL safety validation
// ---------------------------------------------------------------------------

fn is_private_url(url: &str) -> bool {
    let lower = url.to_lowercase();

    let host_part = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);

    let host = host_part
        .split('/')
        .next()
        .unwrap_or(host_part)
        .split(':')
        .next()
        .unwrap_or(host_part);

    matches!(
        host,
        "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]"
    ) || host.starts_with("10.")
        || host.starts_with("192.168.")
        || (host.starts_with("172.")
            && host
                .split('.')
                .nth(1)
                .and_then(|s| s.parse::<u8>().ok())
                .is_some_and(|n| (16..=31).contains(&n)))
}

// ---------------------------------------------------------------------------
// JSON path extraction — supports dot notation and array indexing
// ---------------------------------------------------------------------------

/// Navigate a JSON value by a path like "bpi.USD.rate_float" or "periods[0].temperature"
///
/// Supports:
///   - Dot-separated keys: "data.price"
///   - Array indexing: "periods[0].temperature"
///   - Top-level keys: "price"
fn extract_json_path<'a>(json: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if path.is_empty() {
        return Some(json);
    }

    let mut current = json;

    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }

        // Check for array indexing: "periods[0]"
        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = &segment[bracket_pos + 1..segment.len() - 1]; // strip [ and ]

            // Navigate to the key first (if non-empty)
            if !key.is_empty() {
                current = current.get(key)?;
            }

            // Then index into the array
            let idx: usize = idx_str.parse().ok()?;
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }

    Some(current)
}

// ---------------------------------------------------------------------------
// Convert extracted JSON value to the requested type and ABI-encode
// ---------------------------------------------------------------------------

fn encode_json_value(value: &JsonValue, response_type: u8) -> Result<Vec<u8>, String> {
    match response_type {
        // 0 = raw bytes (JSON value as UTF-8)
        0 => {
            let raw_str = match value {
                JsonValue::String(s) => s.clone(),
                other => other.to_string(),
            };
            let raw: Bytes = raw_str.into_bytes().into();
            Ok((raw,).abi_encode_params())
        }
        // 1 = uint256
        1 => {
            let num = match value {
                JsonValue::Number(n) => {
                    if let Some(u) = n.as_u64() {
                        u
                    } else if let Some(f) = n.as_f64() {
                        // Truncate float to integer (e.g. 72.5 → 72)
                        if f < 0.0 {
                            return Err(format!("negative number cannot be uint256: {f}"));
                        }
                        f as u64
                    } else {
                        return Err(format!("cannot convert number to uint256: {n}"));
                    }
                }
                JsonValue::String(s) => {
                    // Try parsing string as number (some APIs return numbers as strings)
                    let trimmed = s.trim().replace(',', "");
                    if let Ok(u) = trimmed.parse::<u64>() {
                        u
                    } else if let Ok(f) = trimmed.parse::<f64>() {
                        if f < 0.0 {
                            return Err(format!("negative number cannot be uint256: {f}"));
                        }
                        f as u64
                    } else {
                        return Err(format!("cannot parse string as uint256: {s}"));
                    }
                }
                JsonValue::Bool(b) => {
                    if *b { 1 } else { 0 }
                }
                _ => return Err(format!("cannot convert {value} to uint256")),
            };
            Ok((U256::from(num),).abi_encode_params())
        }
        // 2 = string
        2 => {
            let s = match value {
                JsonValue::String(s) => s.clone(),
                other => other.to_string(),
            };
            Ok((s,).abi_encode_params())
        }
        // 3 = bool
        3 => {
            let b = match value {
                JsonValue::Bool(b) => *b,
                JsonValue::Number(n) => n.as_u64().map(|v| v != 0).unwrap_or(false),
                JsonValue::String(s) => {
                    matches!(s.to_lowercase().as_str(), "true" | "1" | "yes")
                }
                JsonValue::Null => false,
                _ => true, // objects/arrays are truthy
            };
            Ok((b,).abi_encode_params())
        }
        _ => Err(format!("invalid responseType: {response_type}")),
    }
}

// ---------------------------------------------------------------------------
// Perform the actual HTTP call
// ---------------------------------------------------------------------------

fn perform_http_call(request: &ApiRequest) -> Result<JsonValue, String> {
    // reqwest::blocking can panic inside a tokio runtime, so we use
    // tokio::task::block_in_place to allow blocking within the async context.
    let result = tokio::task::block_in_place(|| {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(HTTP_TIMEOUT_MS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("HTTP client build error: {e}"))?;

        // Build the request
        let mut req_builder = match request.method.as_str() {
            "GET" => client.get(&request.url),
            "POST" => client.post(&request.url),
            _ => return Err(format!("unsupported method: {}", request.method)),
        };

        // Always set a User-Agent — many APIs (weather.gov, CoinGecko) reject
        // requests without one.
        req_builder = req_builder.header("User-Agent", "ExternEVM/0.4.0");

        // Parse and apply headers from bytes (expected to be JSON: {"Key": "Value", ...})
        if !request.headers.is_empty() {
            match serde_json::from_slice::<JsonValue>(&request.headers) {
                Ok(JsonValue::Object(map)) => {
                    for (key, val) in map {
                        if let JsonValue::String(v) = val {
                            req_builder = req_builder.header(&key, &v);
                        }
                    }
                }
                Ok(_) => {
                    return Err("headers must be a JSON object".to_string());
                }
                Err(e) => {
                    return Err(format!("failed to parse headers JSON: {e}"));
                }
            }
        }

        // Set body for POST
        if request.method == "POST" && !request.body.is_empty() {
            req_builder = req_builder.body(request.body.to_vec());
        }

        // Send the request
        let response = req_builder
            .send()
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }

        // Read response body with size limit
        let body_bytes = response
            .bytes()
            .map_err(|e| format!("failed to read response body: {e}"))?;

        if body_bytes.len() > MAX_RESPONSE_SIZE {
            return Err(format!(
                "response size {} exceeds max {}",
                body_bytes.len(),
                MAX_RESPONSE_SIZE
            ));
        }

        // Parse JSON
        let json: JsonValue = serde_json::from_slice(&body_bytes)
            .map_err(|e| format!("failed to parse response JSON: {e}"))?;

        Ok(json)
    });

    result
}

// ---------------------------------------------------------------------------
// Precompile entry point
// ---------------------------------------------------------------------------

fn api_call_id() -> PrecompileId {
    PrecompileId::Custom("API_CALL".into())
}

fn api_call_precompile(input: PrecompileInput<'_>) -> PrecompileResult {
    let gas_used = API_CALL_GAS;

    if input.gas < gas_used {
        return Ok(PrecompileOutput::halt(PrecompileHalt::OutOfGas, input.reservoir));
    }

    // Backward compatibility: empty input → uint256(1234)
    if input.data.is_empty() {
        let output = (U256::from(1234u64),).abi_encode_params();
        return Ok(PrecompileOutput::new(gas_used, output.into(), input.reservoir));
    }

    // --- Decode ABI-encoded ApiRequest ---
    let request = match ApiRequest::abi_decode(input.data) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("[ExternEVM] ABI decode error: {e}");
            return Ok(PrecompileOutput::halt(
                PrecompileHalt::Other("API_CALL: failed to decode ApiRequest".into()),
                input.reservoir,
            ));
        }
    };

    eprintln!("[ExternEVM] API_CALL decoded:");
    eprintln!("  url:          {}", request.url);
    eprintln!("  method:       {}", request.method);
    eprintln!("  headers:      {} bytes", request.headers.len());
    eprintln!("  body:         {} bytes", request.body.len());
    eprintln!("  responsePath: {}", request.responsePath);
    eprintln!("  responseType: {}", request.responseType);

    // --- Validation ---
    if request.url.is_empty() {
        eprintln!("[ExternEVM] ERROR: url is empty");
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: url is empty".into()),
            input.reservoir,
        ));
    }

    if !request.url.starts_with("http://") && !request.url.starts_with("https://") {
        eprintln!("[ExternEVM] ERROR: url must start with http:// or https://");
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: url must start with http:// or https://".into()),
            input.reservoir,
        ));
    }

    if is_private_url(&request.url) {
        eprintln!("[ExternEVM] ERROR: private/loopback URLs are blocked");
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: private/loopback URLs are blocked".into()),
            input.reservoir,
        ));
    }

    if request.method != "GET" && request.method != "POST" {
        eprintln!("[ExternEVM] ERROR: invalid method '{}'", request.method);
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: method must be GET or POST".into()),
            input.reservoir,
        ));
    }

    if request.responseType > 3 {
        eprintln!("[ExternEVM] ERROR: invalid responseType {}", request.responseType);
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: responseType must be 0-3".into()),
            input.reservoir,
        ));
    }

    if request.body.len() > MAX_BODY_SIZE {
        eprintln!(
            "[ExternEVM] ERROR: body size {} exceeds max {}",
            request.body.len(),
            MAX_BODY_SIZE
        );
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: body exceeds max size".into()),
            input.reservoir,
        ));
    }

    // --- Milestone 4: Real HTTP call ---
    let json = match perform_http_call(&request) {
        Ok(json) => {
            eprintln!("[ExternEVM] HTTP response received, parsing...");
            json
        }
        Err(e) => {
            eprintln!("[ExternEVM] HTTP error: {e}");
            return Ok(PrecompileOutput::halt(
                PrecompileHalt::Other(format!("API_CALL: {e}").into()),
                input.reservoir,
            ));
        }
    };

    // --- Extract value at responsePath ---
    let extracted = match extract_json_path(&json, &request.responsePath) {
        Some(val) => val,
        None => {
            eprintln!(
                "[ExternEVM] ERROR: responsePath '{}' not found in JSON",
                request.responsePath
            );
            eprintln!("[ExternEVM] JSON body: {json}");
            return Ok(PrecompileOutput::halt(
                PrecompileHalt::Other(
                    format!("API_CALL: responsePath '{}' not found", request.responsePath).into(),
                ),
                input.reservoir,
            ));
        }
    };

    eprintln!(
        "[ExternEVM] Extracted value at '{}': {}",
        request.responsePath, extracted
    );

    // --- Encode to the requested type ---
    let output_bytes = match encode_json_value(extracted, request.responseType) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("[ExternEVM] Encoding error: {e}");
            return Ok(PrecompileOutput::halt(
                PrecompileHalt::Other(format!("API_CALL: {e}").into()),
                input.reservoir,
            ));
        }
    };

    eprintln!(
        "[ExternEVM] Returning real response ({} bytes, responseType={})",
        output_bytes.len(),
        request.responseType
    );

    Ok(PrecompileOutput::new(gas_used, output_bytes.into(), input.reservoir))
}

fn api_call_dyn_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(api_call_id(), api_call_precompile)
}

pub fn inject_api_call_precompile(precompiles: &mut PrecompilesMap) {
    precompiles.apply_precompile(&API_CALL_ADDRESS, |_| Some(api_call_dyn_precompile()));
}

// ---------------------------------------------------------------------------
// ExternEvmFactory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ExternEvmFactory {
    inner: alloy_evm::EthEvmFactory,
}

impl ExternEvmFactory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EvmFactory for ExternEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> =
        <alloy_evm::EthEvmFactory as EvmFactory>::Evm<DB, I>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv,
    ) -> Self::Evm<DB, NoOpInspector> {
        let mut evm = self.inner.create_evm(db, input);
        inject_api_call_precompile(evm.precompiles_mut());
        evm
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let mut evm = self.inner.create_evm_with_inspector(db, input, inspector);
        inject_api_call_precompile(evm.precompiles_mut());
        evm
    }
}