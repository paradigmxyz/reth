//! ExternEVM custom EvmFactory.
//!
//! Wraps the standard `EthEvmFactory` and injects the API_CALL precompile
//! at address 0x00000000000000000000000000000000000000AA.
//!
//! Milestone 1: any input → uint256(1234)
//! Milestone 3: decode ApiRequest (raw URL mode), validate, return mock responses
//! Milestone 4 (future): real HTTP calls via reqwest

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
// Mock response logic — Milestone 3 only
// ---------------------------------------------------------------------------

fn mock_uint_value(url: &str) -> u64 {
    let lower = url.to_lowercase();
    if lower.contains("weather") {
        72
    } else if lower.contains("bitcoin") || lower.contains("btc") {
        104_000
    } else if lower.contains("gold") {
        2_340
    } else if lower.contains("price") {
        42_000
    } else {
        1234
    }
}

fn mock_string_value(url: &str) -> String {
    let lower = url.to_lowercase();
    if lower.contains("weather") {
        "Sunny, 72°F".to_string()
    } else if lower.contains("joke") || lower.contains("random") {
        "Why did the smart contract go to therapy? Too many trust issues.".to_string()
    } else if lower.contains("bitcoin") || lower.contains("btc") {
        "104000".to_string()
    } else if lower.contains("gold") {
        "2340".to_string()
    } else {
        "ExternEVM mock response".to_string()
    }
}

// ---------------------------------------------------------------------------
// ABI encoding helpers — wrap in tuple for abi_encode_params
// ---------------------------------------------------------------------------

fn encode_response(response_type: u8, url: &str) -> Vec<u8> {
    match response_type {
        0 => {
            let raw: Bytes = mock_string_value(url).into_bytes().into();
            (raw,).abi_encode_params()
        }
        1 => {
            (U256::from(mock_uint_value(url)),).abi_encode_params()
        }
        2 => {
            (mock_string_value(url),).abi_encode_params()
        }
        3 => {
            (mock_uint_value(url) != 0,).abi_encode_params()
        }
        _ => unreachable!(),
    }
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

    // --- Milestone 3: Decode ABI-encoded ApiRequest ---
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
        eprintln!("[ExternEVM] ERROR: body size {} exceeds max {}", request.body.len(), MAX_BODY_SIZE);
        return Ok(PrecompileOutput::halt(
            PrecompileHalt::Other("API_CALL: body exceeds max size".into()),
            input.reservoir,
        ));
    }

    // --- Mock response ---
    let output_bytes = encode_response(request.responseType, &request.url);
    eprintln!(
        "[ExternEVM] Returning mock response ({} bytes, responseType={})",
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