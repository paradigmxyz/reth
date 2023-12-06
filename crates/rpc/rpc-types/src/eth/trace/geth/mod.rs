#![allow(missing_docs)]
//! Geth tracing types

use crate::{state::StateOverride, BlockOverrides};
use alloy_primitives::{Bytes, B256, U256};
use serde::{de::DeserializeOwned, ser::SerializeMap, Deserialize, Serialize, Serializer};
use std::{collections::BTreeMap, time::Duration};

// re-exports
pub use self::{
    call::{CallConfig, CallFrame, CallLogFrame},
    four_byte::FourByteFrame,
    noop::NoopFrame,
    pre_state::{
        AccountChangeKind, AccountState, DiffMode, DiffStateKind, PreStateConfig, PreStateFrame,
        PreStateMode,
    },
};

mod call;
mod four_byte;
mod noop;
mod pre_state;

/// Result type for geth style transaction trace
pub type TraceResult = crate::trace::common::TraceResult<GethTrace, String>;

/// blockTraceResult represents the results of tracing a single block when an entire chain is being
/// traced. ref <https://github.com/ethereum/go-ethereum/blob/ee530c0d5aa70d2c00ab5691a89ab431b73f8165/eth/tracers/api.go#L218-L222>
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockTraceResult {
    /// Block number corresponding to the trace task
    pub block: U256,
    /// Block hash corresponding to the trace task
    pub hash: B256,
    /// Trace results produced by the trace task
    pub traces: Vec<TraceResult>,
}

/// Geth Default struct log trace frame
///
/// <https://github.com/ethereum/go-ethereum/blob/a9ef135e2dd53682d106c6a2aede9187026cc1de/eth/tracers/logger/logger.go#L406-L411>
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultFrame {
    /// Whether the transaction failed
    pub failed: bool,
    /// How much gas was used.
    pub gas: u64,
    /// Output of the transaction
    #[serde(serialize_with = "crate::serde_helpers::serialize_hex_string_no_prefix")]
    pub return_value: Bytes,
    /// Recorded traces of the transaction
    pub struct_logs: Vec<StructLog>,
}

/// Represents a struct log entry in a trace
///
/// <https://github.com/ethereum/go-ethereum/blob/366d2169fbc0e0f803b68c042b77b6b480836dbc/eth/tracers/logger/logger.go#L413-L426>
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructLog {
    /// program counter
    pub pc: u64,
    /// opcode to be executed
    pub op: String,
    /// remaining gas
    pub gas: u64,
    /// cost for executing op
    #[serde(rename = "gasCost")]
    pub gas_cost: u64,
    /// ref <https://github.com/ethereum/go-ethereum/blob/366d2169fbc0e0f803b68c042b77b6b480836dbc/eth/tracers/logger/logger.go#L450-L452>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<Vec<String>>,
    /// Size of memory.
    #[serde(default, rename = "memSize", skip_serializing_if = "Option::is_none")]
    pub memory_size: Option<u64>,
    /// EVM stack
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<Vec<U256>>,
    /// Last call's return data. Enabled via enableReturnData
    #[serde(default, rename = "returnData", skip_serializing_if = "Option::is_none")]
    pub return_data: Option<Bytes>,
    /// Storage slots of current contract read from and written to. Only emitted for SLOAD and
    /// SSTORE. Disabled via disableStorage
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_string_storage_map_opt"
    )]
    pub storage: Option<BTreeMap<B256, B256>>,
    /// Current call depth
    pub depth: u64,
    /// Refund counter
    #[serde(default, rename = "refund", skip_serializing_if = "Option::is_none")]
    pub refund_counter: Option<u64>,
    /// Error message if any
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tracing response objects
///
/// Note: This deserializes untagged, so it's possible that a custom javascript tracer response
/// matches another variant, for example a js tracer that returns `{}` would be deserialized as
/// [GethTrace::NoopTracer]
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum GethTrace {
    /// The response for the default struct log tracer
    Default(DefaultFrame),
    /// The response for call tracer
    CallTracer(CallFrame),
    /// The response for four byte tracer
    FourByteTracer(FourByteFrame),
    /// The response for pre-state byte tracer
    PreStateTracer(PreStateFrame),
    /// An empty json response
    NoopTracer(NoopFrame),
    /// Any other trace response, such as custom javascript response objects
    JS(serde_json::Value),
}

impl From<DefaultFrame> for GethTrace {
    fn from(value: DefaultFrame) -> Self {
        GethTrace::Default(value)
    }
}

impl From<FourByteFrame> for GethTrace {
    fn from(value: FourByteFrame) -> Self {
        GethTrace::FourByteTracer(value)
    }
}

impl From<CallFrame> for GethTrace {
    fn from(value: CallFrame) -> Self {
        GethTrace::CallTracer(value)
    }
}

impl From<PreStateFrame> for GethTrace {
    fn from(value: PreStateFrame) -> Self {
        GethTrace::PreStateTracer(value)
    }
}

impl From<NoopFrame> for GethTrace {
    fn from(value: NoopFrame) -> Self {
        GethTrace::NoopTracer(value)
    }
}

/// Available built-in tracers
///
/// See <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub enum GethDebugBuiltInTracerType {
    /// The 4byteTracer collects the function selectors of every function executed in the lifetime
    /// of a transaction, along with the size of the supplied call data. The result is a
    /// [FourByteFrame] where the keys are SELECTOR-CALLDATASIZE and the values are number of
    /// occurrences of this key.
    #[serde(rename = "4byteTracer")]
    FourByteTracer,
    /// The callTracer tracks all the call frames executed during a transaction, including depth 0.
    /// The result will be a nested list of call frames, resembling how EVM works. They form a tree
    /// with the top-level call at root and sub-calls as children of the higher levels.
    #[serde(rename = "callTracer")]
    CallTracer,
    /// The prestate tracer has two modes: prestate and diff. The prestate mode returns the
    /// accounts necessary to execute a given transaction. diff mode returns the differences
    /// between the transaction's pre and post-state (i.e. what changed because the transaction
    /// happened). The prestateTracer defaults to prestate mode. It reexecutes the given
    /// transaction and tracks every part of state that is touched. This is similar to the concept
    /// of a stateless witness, the difference being this tracer doesn't return any cryptographic
    /// proof, rather only the trie leaves. The result is an object. The keys are addresses of
    /// accounts.
    #[serde(rename = "prestateTracer")]
    PreStateTracer,
    /// This tracer is noop. It returns an empty object and is only meant for testing the setup.
    #[serde(rename = "noopTracer")]
    NoopTracer,
}

/// Available tracers
///
/// See <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers> and <https://geth.ethereum.org/docs/developers/evm-tracing/custom-tracer>
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum GethDebugTracerType {
    /// built-in tracer
    BuiltInTracer(GethDebugBuiltInTracerType),
    /// custom JS tracer
    JsTracer(String),
}

impl From<GethDebugBuiltInTracerType> for GethDebugTracerType {
    fn from(value: GethDebugBuiltInTracerType) -> Self {
        GethDebugTracerType::BuiltInTracer(value)
    }
}

/// Configuration of the tracer
///
/// This is a simple wrapper around serde_json::Value.
/// with helpers for deserializing tracer configs.
#[derive(Debug, PartialEq, Eq, Clone, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct GethDebugTracerConfig(pub serde_json::Value);

// === impl GethDebugTracerConfig ===

impl GethDebugTracerConfig {
    /// Returns if this is a null object
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// Consumes the config and tries to deserialize it into the given type.
    pub fn from_value<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.0)
    }

    /// Returns the [CallConfig] if it is a call config.
    pub fn into_call_config(self) -> Result<CallConfig, serde_json::Error> {
        if self.0.is_null() {
            return Ok(Default::default())
        }
        self.from_value()
    }

    /// Returns the raw json value
    pub fn into_json(self) -> serde_json::Value {
        self.0
    }

    /// Returns the [PreStateConfig] if it is a call config.
    pub fn into_pre_state_config(self) -> Result<PreStateConfig, serde_json::Error> {
        if self.0.is_null() {
            return Ok(Default::default())
        }
        self.from_value()
    }
}

impl From<serde_json::Value> for GethDebugTracerConfig {
    fn from(value: serde_json::Value) -> Self {
        GethDebugTracerConfig(value)
    }
}

/// Bindings for additional `debug_traceTransaction` options
///
/// See <https://geth.ethereum.org/docs/rpc/ns-debug#debug_tracetransaction>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GethDebugTracingOptions {
    /// The common tracing options
    #[serde(default, flatten)]
    pub config: GethDefaultTracingOptions,
    /// The custom tracer to use.
    ///
    /// If `None` then the default structlog tracer is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracer: Option<GethDebugTracerType>,
    /// Config specific to given `tracer`.
    ///
    /// Note default struct logger config are historically embedded in main object.
    ///
    /// tracerConfig is slated for Geth v1.11.0
    /// See <https://github.com/ethereum/go-ethereum/issues/26513>
    ///
    /// This could be [CallConfig] or [PreStateConfig] depending on the tracer.
    #[serde(default, skip_serializing_if = "GethDebugTracerConfig::is_null")]
    pub tracer_config: GethDebugTracerConfig,
    /// A string of decimal integers that overrides the JavaScript-based tracing calls default
    /// timeout of 5 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
}

impl GethDebugTracingOptions {
    /// Sets the tracer to use
    pub fn with_tracer(mut self, tracer: GethDebugTracerType) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Sets the timeout to use for tracing
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(format!("{}ms", duration.as_millis()));
        self
    }

    /// Configures a [CallConfig]
    pub fn call_config(mut self, config: CallConfig) -> Self {
        self.tracer_config =
            GethDebugTracerConfig(serde_json::to_value(config).expect("is serializable"));
        self
    }

    /// Configures a [PreStateConfig]
    pub fn prestate_config(mut self, config: PreStateConfig) -> Self {
        self.tracer_config =
            GethDebugTracerConfig(serde_json::to_value(config).expect("is serializable"));
        self
    }
}

/// Default tracing options for the struct looger.
///
/// These are all known general purpose tracer options that may or not be supported by a given
/// tracer. For example, the `enableReturnData` option is a noop on regular
/// `debug_trace{Transaction,Block}` calls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GethDefaultTracingOptions {
    /// enable memory capture
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_memory: Option<bool>,
    /// Disable memory capture
    ///
    /// This is the opposite of `enable_memory`.
    ///
    /// Note: memory capture used to be enabled by default on geth, but has since been flipped <https://github.com/ethereum/go-ethereum/pull/23558> and is now disabled by default.
    /// However, at the time of writing this, erigon still defaults to enabled and supports the
    /// `disableMemory` option. So we keep this option for compatibility, but if it's missing
    /// OR `enableMemory` is present `enableMemory` takes precedence.
    ///
    /// See also <https://github.com/paradigmxyz/reth/issues/3033>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_memory: Option<bool>,
    /// disable stack capture
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_stack: Option<bool>,
    /// Disable storage capture
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_storage: Option<bool>,
    /// Enable return data capture
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_return_data: Option<bool>,
    /// Disable return data capture
    ///
    /// This is the opposite of `enable_return_data`, and only supported for compatibility reasons.
    /// See also `disable_memory`.
    ///
    /// If `enable_return_data` is present, `enable_return_data` always takes precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_return_data: Option<bool>,
    /// print output during capture end
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    /// maximum length of output, but zero means unlimited
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

impl GethDefaultTracingOptions {
    /// Enables memory capture.
    pub fn enable_memory(self) -> Self {
        self.with_enable_memory(true)
    }

    /// Disables memory capture.
    pub fn disable_memory(self) -> Self {
        self.with_disable_memory(true)
    }

    /// Disables stack capture.
    pub fn disable_stack(self) -> Self {
        self.with_disable_stack(true)
    }

    /// Disables storage capture.
    pub fn disable_storage(self) -> Self {
        self.with_disable_storage(true)
    }

    /// Enables return data capture.
    pub fn enable_return_data(self) -> Self {
        self.with_enable_return_data(true)
    }

    /// Disables return data capture.
    pub fn disable_return_data(self) -> Self {
        self.with_disable_return_data(true)
    }

    /// Enables debug mode.
    pub fn debug(self) -> Self {
        self.with_debug(true)
    }

    /// Sets the enable_memory field.
    pub fn with_enable_memory(mut self, enable: bool) -> Self {
        self.enable_memory = Some(enable);
        self
    }

    /// Sets the disable_memory field.
    pub fn with_disable_memory(mut self, disable: bool) -> Self {
        self.disable_memory = Some(disable);
        self
    }

    /// Sets the disable_stack field.
    pub fn with_disable_stack(mut self, disable: bool) -> Self {
        self.disable_stack = Some(disable);
        self
    }

    /// Sets the disable_storage field.
    pub fn with_disable_storage(mut self, disable: bool) -> Self {
        self.disable_storage = Some(disable);
        self
    }

    /// Sets the enable_return_data field.
    pub fn with_enable_return_data(mut self, enable: bool) -> Self {
        self.enable_return_data = Some(enable);
        self
    }

    /// Sets the disable_return_data field.
    pub fn with_disable_return_data(mut self, disable: bool) -> Self {
        self.disable_return_data = Some(disable);
        self
    }

    /// Sets the debug field.
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = Some(debug);
        self
    }

    /// Sets the limit field.
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }
    /// Returns `true` if return data capture is enabled
    pub fn is_return_data_enabled(&self) -> bool {
        self.enable_return_data
            .or_else(|| self.disable_return_data.map(|disable| !disable))
            .unwrap_or(false)
    }

    /// Returns `true` if memory capture is enabled
    pub fn is_memory_enabled(&self) -> bool {
        self.enable_memory.or_else(|| self.disable_memory.map(|disable| !disable)).unwrap_or(false)
    }

    /// Returns `true` if stack capture is enabled
    pub fn is_stack_enabled(&self) -> bool {
        !self.disable_stack.unwrap_or(false)
    }

    /// Returns `true` if storage capture is enabled
    pub fn is_storage_enabled(&self) -> bool {
        !self.disable_storage.unwrap_or(false)
    }
}
/// Bindings for additional `debug_traceCall` options
///
/// See <https://geth.ethereum.org/docs/rpc/ns-debug#debug_tracecall>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GethDebugTracingCallOptions {
    /// All the options
    #[serde(flatten)]
    pub tracing_options: GethDebugTracingOptions,
    /// The state overrides to apply
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_overrides: Option<StateOverride>,
    /// The block overrides to apply
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_overrides: Option<BlockOverrides>,
}

/// Serializes a storage map as a list of key-value pairs _without_ 0x-prefix
fn serialize_string_storage_map_opt<S: Serializer>(
    storage: &Option<BTreeMap<B256, B256>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match storage {
        None => s.serialize_none(),
        Some(storage) => {
            let mut m = s.serialize_map(Some(storage.len()))?;
            for (key, val) in storage.iter() {
                let key = format!("{:?}", key);
                let val = format!("{:?}", val);
                // skip the 0x prefix
                m.serialize_entry(&key.as_str()[2..], &val.as_str()[2..])?;
            }
            m.end()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracer_config() {
        let s = "{\"tracer\": \"callTracer\"}";
        let opts = serde_json::from_str::<GethDebugTracingOptions>(s).unwrap();
        assert_eq!(
            opts.tracer,
            Some(GethDebugTracerType::BuiltInTracer(GethDebugBuiltInTracerType::CallTracer))
        );
        let _call_config = opts.tracer_config.clone().into_call_config().unwrap();
        let _prestate_config = opts.tracer_config.into_pre_state_config().unwrap();
    }

    #[test]
    fn test_memory_capture() {
        let mut config = GethDefaultTracingOptions::default();

        // by default false
        assert!(!config.is_memory_enabled());

        config.disable_memory = Some(false);
        // disable == false -> enable
        assert!(config.is_memory_enabled());

        config.enable_memory = Some(false);
        // enable == false -> disable
        assert!(!config.is_memory_enabled());
    }

    #[test]
    fn test_return_data_capture() {
        let mut config = GethDefaultTracingOptions::default();

        // by default false
        assert!(!config.is_return_data_enabled());

        config.disable_return_data = Some(false);
        // disable == false -> enable
        assert!(config.is_return_data_enabled());

        config.enable_return_data = Some(false);
        // enable == false -> disable
        assert!(!config.is_return_data_enabled());
    }

    // <https://etherscan.io/tx/0xd01212e8ab48d2fd2ea9c4f33f8670fd1cf0cfb09d2e3c6ceddfaf54152386e5>
    #[test]
    fn serde_default_frame() {
        let input = include_str!("../../../../test_data/default/structlogs_01.json");
        let _frame: DefaultFrame = serde_json::from_str(input).unwrap();
    }

    #[test]
    fn test_serialize_storage_map() {
        let s = r#"{"pc":3349,"op":"SLOAD","gas":23959,"gasCost":2100,"depth":1,"stack":[],"memory":[],"storage":{"6693dabf5ec7ab1a0d1c5bc58451f85d5e44d504c9ffeb75799bfdb61aa2997a":"0000000000000000000000000000000000000000000000000000000000000000"}}"#;
        let log: StructLog = serde_json::from_str(s).unwrap();
        let val = serde_json::to_value(&log).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        similar_asserts::assert_eq!(input, val);
    }
}
