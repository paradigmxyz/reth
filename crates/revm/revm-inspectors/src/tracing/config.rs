/// Gives guidance to the [TracingInspector](crate::tracing::TracingInspector).
///
/// Use [TraceInspectorConfig::default_parity] or [TraceInspectorConfig::default_geth] to get the
/// default configs for specific styles of traces.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TraceInspectorConfig {
    /// Whether to record every individual opcode level step.
    pub record_steps: bool,
    /// Whether to record individual memory snapshots.
    pub record_memory_snapshots: bool,
    /// Whether to record individual stack snapshots.
    pub record_stack_snapshots: bool,
    /// Whether to record state diffs.
    pub record_state_diff: bool,
}

impl TraceInspectorConfig {
    /// Returns a config with everything enabled.
    pub const fn all() -> Self {
        Self {
            record_steps: true,
            record_memory_snapshots: true,
            record_stack_snapshots: true,
            record_state_diff: false,
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
        }
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
}
