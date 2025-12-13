//! Engine API capabilities.

use std::collections::HashSet;
use tracing::warn;

/// All Engine API capabilities supported by Reth (Ethereum mainnet).
///
/// See <https://github.com/ethereum/execution-apis/tree/main/src/engine> for updates.
pub const CAPABILITIES: &[&str] = &[
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_forkchoiceUpdatedV3",
    "engine_getClientVersionV1",
    "engine_getPayloadV1",
    "engine_getPayloadV2",
    "engine_getPayloadV3",
    "engine_getPayloadV4",
    "engine_getPayloadV5",
    "engine_newPayloadV1",
    "engine_newPayloadV2",
    "engine_newPayloadV3",
    "engine_newPayloadV4",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByRangeV1",
    "engine_getBlobsV1",
    "engine_getBlobsV2",
];

/// Engine API capabilities set.
#[derive(Debug, Clone)]
pub struct EngineCapabilities {
    inner: HashSet<String>,
}

impl EngineCapabilities {
    /// Creates from an iterator of capability strings.
    pub fn new(capabilities: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { inner: capabilities.into_iter().map(Into::into).collect() }
    }

    /// Returns the capabilities as a list of strings.
    pub fn list(&self) -> Vec<String> {
        self.inner.iter().cloned().collect()
    }

    /// Returns a reference to the inner set.
    pub const fn as_set(&self) -> &HashSet<String> {
        &self.inner
    }
}

impl Default for EngineCapabilities {
    fn default() -> Self {
        Self::new(CAPABILITIES.iter().copied())
    }
}

/// Logs warnings if CL and EL capabilities don't match.
///
/// Called during `engine_exchangeCapabilities` to warn operators about
/// version mismatches between the consensus layer and execution layer.
pub fn log_capability_mismatches(cl_capabilities: &[String], el_capabilities: &EngineCapabilities) {
    let cl_set: HashSet<&str> = cl_capabilities.iter().map(|s| s.as_str()).collect();
    let el_set: HashSet<&str> = el_capabilities.inner.iter().map(|s| s.as_str()).collect();

    // CL has methods EL doesn't support
    let mut el_missing: Vec<_> = cl_set.difference(&el_set).copied().collect();
    if !el_missing.is_empty() {
        el_missing.sort();
        warn!(
            target: "rpc::engine",
            missing = ?el_missing,
            "CL supports Engine API methods that Reth doesn't. Consider upgrading Reth."
        );
    }

    // EL has methods CL doesn't support
    let mut cl_missing: Vec<_> = el_set.difference(&cl_set).copied().collect();
    if !cl_missing.is_empty() {
        cl_missing.sort();
        warn!(
            target: "rpc::engine",
            missing = ?cl_missing,
            "Reth supports Engine API methods that CL doesn't. Consider upgrading your consensus client."
        );
    }
}
