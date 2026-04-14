//! Engine API capabilities.

use std::collections::HashSet;
use tracing::warn;

/// Critical Engine API method prefixes that warrant warnings on capability mismatches.
///
/// These are essential for block production and chain synchronization. Missing support
/// for these methods indicates a significant version mismatch that operators should address.
const CRITICAL_METHOD_PREFIXES: &[&str] =
    &["engine_forkchoiceUpdated", "engine_getPayload", "engine_newPayload"];

/// All Engine API capabilities supported by Reth (Ethereum mainnet).
///
/// See <https://github.com/ethereum/execution-apis/tree/main/src/engine> for updates.
pub const CAPABILITIES: &[&str] = &[
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_forkchoiceUpdatedV3",
    "engine_forkchoiceUpdatedV4",
    "engine_getClientVersionV1",
    "engine_getPayloadV1",
    "engine_getPayloadV2",
    "engine_getPayloadV3",
    "engine_getPayloadV4",
    "engine_getPayloadV5",
    "engine_getPayloadV6",
    "engine_newPayloadV1",
    "engine_newPayloadV2",
    "engine_newPayloadV3",
    "engine_newPayloadV4",
    "engine_newPayloadV5",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByHashV2",
    "engine_getPayloadBodiesByRangeV1",
    "engine_getPayloadBodiesByRangeV2",
    "engine_getBlobsV1",
    "engine_getBlobsV2",
    "engine_getBlobsV3",
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

    /// Compares CL capabilities with this EL's capabilities and returns any mismatches.
    ///
    /// Called during `engine_exchangeCapabilities` to detect version mismatches
    /// between the consensus layer and execution layer.
    pub fn get_capability_mismatches(&self, cl_capabilities: &[String]) -> CapabilityMismatches {
        let cl_set: HashSet<&str> = cl_capabilities.iter().map(String::as_str).collect();

        // CL has methods EL doesn't support
        let mut missing_in_el: Vec<_> = cl_capabilities
            .iter()
            .filter(|cap| !self.inner.contains(cap.as_str()))
            .cloned()
            .collect();
        missing_in_el.sort_unstable();

        // EL has methods CL doesn't support
        let mut missing_in_cl: Vec<_> =
            self.inner.iter().filter(|cap| !cl_set.contains(cap.as_str())).cloned().collect();
        missing_in_cl.sort_unstable();

        CapabilityMismatches { missing_in_el, missing_in_cl }
    }

    /// Logs warnings if CL and EL capabilities don't match for critical methods.
    ///
    /// Called during `engine_exchangeCapabilities` to warn operators about
    /// version mismatches between the consensus layer and execution layer.
    ///
    /// Only warns about critical methods (`engine_forkchoiceUpdated`, `engine_getPayload`,
    /// `engine_newPayload`) that are essential for block production and chain synchronization.
    /// Non-critical methods like `engine_getBlobs` are not warned about since not all
    /// clients support them.
    pub fn log_capability_mismatches(&self, cl_capabilities: &[String]) {
        let mismatches = self.get_capability_mismatches(cl_capabilities);

        let critical_missing_in_el: Vec<_> =
            mismatches.missing_in_el.iter().filter(|m| is_critical_method(m)).cloned().collect();

        let critical_missing_in_cl: Vec<_> =
            mismatches.missing_in_cl.iter().filter(|m| is_critical_method(m)).cloned().collect();

        if !critical_missing_in_el.is_empty() {
            warn!(
                target: "rpc::engine",
                missing = ?critical_missing_in_el,
                "CL supports Engine API methods that Reth doesn't. Consider upgrading Reth."
            );
        }

        if !critical_missing_in_cl.is_empty() {
            warn!(
                target: "rpc::engine",
                missing = ?critical_missing_in_cl,
                "Reth supports Engine API methods that CL doesn't. Consider upgrading your consensus client."
            );
        }
    }
}

/// Returns `true` if the method is critical for block production and chain synchronization.
fn is_critical_method(method: &str) -> bool {
    CRITICAL_METHOD_PREFIXES.iter().any(|prefix| {
        method.starts_with(prefix) &&
            method[prefix.len()..]
                .strip_prefix('V')
                .is_some_and(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))
    })
}

impl Default for EngineCapabilities {
    fn default() -> Self {
        Self::new(CAPABILITIES.iter().copied())
    }
}

/// Result of comparing CL and EL capabilities.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CapabilityMismatches {
    /// Methods supported by CL but not by EL (Reth).
    /// Operators should consider upgrading Reth.
    pub missing_in_el: Vec<String>,
    /// Methods supported by EL (Reth) but not by CL.
    /// Operators should consider upgrading their consensus client.
    pub missing_in_cl: Vec<String>,
}

impl CapabilityMismatches {
    /// Returns `true` if there are no mismatches.
    pub const fn is_empty(&self) -> bool {
        self.missing_in_el.is_empty() && self.missing_in_cl.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_mismatches() {
        let el = EngineCapabilities::new(["method_a", "method_b"]);
        let cl = vec!["method_a".to_string(), "method_b".to_string()];

        let result = el.get_capability_mismatches(&cl);
        assert!(result.is_empty());
    }

    #[test]
    fn test_cl_has_extra_methods() {
        let el = EngineCapabilities::new(["method_a"]);
        let cl = vec!["method_a".to_string(), "method_b".to_string()];

        let result = el.get_capability_mismatches(&cl);
        assert_eq!(result.missing_in_el, vec!["method_b"]);
        assert!(result.missing_in_cl.is_empty());
    }

    #[test]
    fn test_el_has_extra_methods() {
        let el = EngineCapabilities::new(["method_a", "method_b"]);
        let cl = vec!["method_a".to_string()];

        let result = el.get_capability_mismatches(&cl);
        assert!(result.missing_in_el.is_empty());
        assert_eq!(result.missing_in_cl, vec!["method_b"]);
    }

    #[test]
    fn test_both_have_extra_methods() {
        let el = EngineCapabilities::new(["method_a", "method_c"]);
        let cl = vec!["method_a".to_string(), "method_b".to_string()];

        let result = el.get_capability_mismatches(&cl);
        assert_eq!(result.missing_in_el, vec!["method_b"]);
        assert_eq!(result.missing_in_cl, vec!["method_c"]);
    }

    #[test]
    fn test_results_are_sorted() {
        let el = EngineCapabilities::new(["z_method", "a_method"]);
        let cl = vec!["z_other".to_string(), "a_other".to_string()];

        let result = el.get_capability_mismatches(&cl);
        assert_eq!(result.missing_in_el, vec!["a_other", "z_other"]);
        assert_eq!(result.missing_in_cl, vec!["a_method", "z_method"]);
    }

    #[test]
    fn test_is_critical_method() {
        assert!(is_critical_method("engine_forkchoiceUpdatedV1"));
        assert!(is_critical_method("engine_forkchoiceUpdatedV3"));
        assert!(is_critical_method("engine_getPayloadV1"));
        assert!(is_critical_method("engine_getPayloadV4"));
        assert!(is_critical_method("engine_newPayloadV1"));
        assert!(is_critical_method("engine_newPayloadV4"));

        assert!(!is_critical_method("engine_getBlobsV1"));
        assert!(!is_critical_method("engine_getBlobsV3"));
        assert!(!is_critical_method("engine_getPayloadBodiesByHashV1"));
        assert!(!is_critical_method("engine_getPayloadBodiesByRangeV1"));
        assert!(!is_critical_method("engine_getClientVersionV1"));
    }
}
