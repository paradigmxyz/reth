use reth_rpc_types::trace::geth::GethDefaultTracingOptions;

/// Gives guidance to the [TracingInspector](crate::tracing::TracingInspector).
///
/// Use [TracingInspectorConfig::default_parity] or [TracingInspectorConfig::default_geth] to get
/// the default configs for specific styles of traces.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TracingInspectorConfig {
    /// Whether to record every individual opcode level step.
    pub record_steps: bool,
    /// Whether to record individual memory snapshots.
    pub record_memory_snapshots: bool,
    /// Whether to record individual stack snapshots.
    pub record_stack_snapshots: bool,
    /// Whether to record state diffs.
    pub record_state_diff: bool,
    /// Whether to ignore precompile calls.
    pub exclude_precompile_calls: bool,
    /// Whether to record individual return data
    pub record_call_return_data: bool,
    /// Whether to record logs
    pub record_logs: bool,
}

impl TracingInspectorConfig {
    /// Returns a config with everything enabled.
    pub const fn all() -> Self {
        Self {
            record_steps: true,
            record_memory_snapshots: true,
            record_stack_snapshots: true,
            record_state_diff: false,
            exclude_precompile_calls: false,
            record_call_return_data: false,
            record_logs: true,
        }
    }

    /// Returns a config for parity style traces.
    ///
    /// This config does _not_ record opcode level traces and is suited for `trace_transaction`
    pub const fn default_parity() -> Self {
        Self {
            record_steps: false,
            record_memory_snapshots: false,
            record_stack_snapshots: false,
            record_state_diff: false,
            exclude_precompile_calls: true,
            record_call_return_data: false,
            record_logs: false,
        }
    }

    /// Returns a config for geth style traces.
    ///
    /// This config does _not_ record opcode level traces and is suited for `debug_traceTransaction`
    pub const fn default_geth() -> Self {
        Self {
            record_steps: true,
            record_memory_snapshots: true,
            record_stack_snapshots: true,
            record_state_diff: true,
            exclude_precompile_calls: false,
            record_call_return_data: false,
            record_logs: false,
        }
    }

    /// Returns a config for geth style traces based on the given [GethDefaultTracingOptions].
    pub fn from_geth_config(config: &GethDefaultTracingOptions) -> Self {
        Self {
            record_memory_snapshots: config.enable_memory.unwrap_or_default(),
            record_stack_snapshots: !config.disable_stack.unwrap_or_default(),
            record_state_diff: !config.disable_storage.unwrap_or_default(),
            ..Self::default_geth()
        }
    }

    /// Configure whether calls to precompiles should be ignored.
    ///
    /// If set to `true`, calls to precompiles without value transfers will be ignored.
    pub fn set_exclude_precompile_calls(mut self, exclude_precompile_calls: bool) -> Self {
        self.exclude_precompile_calls = exclude_precompile_calls;
        self
    }

    /// Configure whether individual opcode level steps should be recorded
    pub fn set_steps(mut self, record_steps: bool) -> Self {
        self.record_steps = record_steps;
        self
    }

    /// Configure whether the tracer should record memory snapshots
    pub fn set_memory_snapshots(mut self, record_memory_snapshots: bool) -> Self {
        self.record_memory_snapshots = record_memory_snapshots;
        self
    }

    /// Configure whether the tracer should record stack snapshots
    pub fn set_stack_snapshots(mut self, record_stack_snapshots: bool) -> Self {
        self.record_stack_snapshots = record_stack_snapshots;
        self
    }

    /// Configure whether the tracer should record state diffs
    pub fn set_state_diffs(mut self, record_state_diff: bool) -> Self {
        self.record_state_diff = record_state_diff;
        self
    }

    /// Configure whether the tracer should record steps and state diffs.
    ///
    /// This is a convenience method for setting both [TracingInspectorConfig::set_steps] and
    /// [TracingInspectorConfig::set_state_diffs] since tracking state diffs requires steps tracing.
    pub fn set_steps_and_state_diffs(mut self, steps_and_diffs: bool) -> Self {
        self.record_steps = steps_and_diffs;
        self.record_state_diff = steps_and_diffs;
        self
    }

    /// Configure whether the tracer should record logs
    pub fn set_record_logs(mut self, record_logs: bool) -> Self {
        self.record_logs = record_logs;
        self
    }
}
