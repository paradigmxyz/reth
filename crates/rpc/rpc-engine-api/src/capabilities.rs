use std::collections::HashSet;

/// The list of all supported Engine capabilities available over the engine endpoint.
pub const CAPABILITIES: &[&str] = &[
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_forkchoiceUpdatedV3",
    "engine_exchangeTransitionConfigurationV1",
    "engine_getClientVersionV1",
    "engine_getPayloadV1",
    "engine_getPayloadV2",
    "engine_getPayloadV3",
    "engine_getPayloadV4",
    "engine_newPayloadV1",
    "engine_newPayloadV2",
    "engine_newPayloadV3",
    "engine_newPayloadV4",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByRangeV1",
    "engine_getBlobsV1",
];

// The list of all supported Engine capabilities available over the engine endpoint.
///
/// Latest spec: Prague
#[derive(Debug, Clone)]
pub struct EngineCapabilities {
    inner: HashSet<String>,
}

impl EngineCapabilities {
    /// Creates a new [`EngineCapabilities`] with a custom list of capabilities.
    pub fn new(capabilities: &[&str]) -> Self {
        Self { inner: capabilities.iter().copied().map(str::to_owned).collect() }
    }

    /// Returns the list of all supported Engine capabilities for Prague spec.
    fn prague() -> Self {
        Self { inner: CAPABILITIES.iter().copied().map(str::to_owned).collect() }
    }

    /// Returns the list of all supported Engine capabilities.
    pub fn list(&self) -> Vec<String> {
        self.inner.iter().cloned().collect()
    }

    /// Adds a new capability to the list.
    pub fn add_capability(&mut self, capability: &str) {
        self.inner.insert(capability.to_owned());
    }

    /// Removes a capability from the list.
    pub fn remove_capability(&mut self, capability: &str) {
        self.inner.remove(capability);
    }
}

impl Default for EngineCapabilities {
    fn default() -> Self {
        Self::prague()
    }
}
