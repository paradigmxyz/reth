use reth_rpc_types::trace::{geth::GethDefaultTracingOptions, parity::TraceType};
use std::collections::HashSet;

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
    pub record_stack_snapshots: StackSnapshotType,
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
            record_stack_snapshots: StackSnapshotType::Full,
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
            record_stack_snapshots: StackSnapshotType::None,
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
            record_stack_snapshots: StackSnapshotType::Full,
            record_state_diff: true,
            exclude_precompile_calls: false,
            record_call_return_data: false,
            record_logs: false,
        }
    }

    /// Returns the [TracingInspectorConfig] depending on the enabled [TraceType]s
    ///
    /// Note: the parity statediffs can be populated entirely via the execution result, so we don't
    /// need statediff recording
    #[inline]
    pub fn from_parity_config(trace_types: &HashSet<TraceType>) -> Self {
        let needs_vm_trace = trace_types.contains(&TraceType::VmTrace);
        let snap_type =
            if needs_vm_trace { StackSnapshotType::Pushes } else { StackSnapshotType::None };
        TracingInspectorConfig::default_parity()
            .set_steps(needs_vm_trace)
            .set_stack_snapshots(snap_type)
            .set_memory_snapshots(needs_vm_trace)
    }

    /// Returns a config for geth style traces based on the given [GethDefaultTracingOptions].
    #[inline]
    pub fn from_geth_config(config: &GethDefaultTracingOptions) -> Self {
        Self {
            record_memory_snapshots: config.enable_memory.unwrap_or_default(),
            record_stack_snapshots: if config.disable_stack.unwrap_or_default() {
                StackSnapshotType::None
            } else {
                StackSnapshotType::Full
            },
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

    /// Configure how the tracer should record stack snapshots
    pub fn set_stack_snapshots(mut self, record_stack_snapshots: StackSnapshotType) -> Self {
        self.record_stack_snapshots = record_stack_snapshots;
        self
    }

    /// Sets state diff recording to true.
    pub fn with_state_diffs(self) -> Self {
        self.set_steps_and_state_diffs(true)
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

/// How much of the stack to record. Nothing, just the items pushed, or the full stack
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StackSnapshotType {
    /// Don't record stack snapshots
    None,
    /// Record only the items pushed to the stack
    Pushes,
    /// Record the full stack
    Full,
}

impl StackSnapshotType {
    /// Returns true if this is the [StackSnapshotType::Full] variant
    #[inline]
    pub fn is_full(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Returns true if this is the [StackSnapshotType::Pushes] variant
    #[inline]
    pub fn is_pushes(self) -> bool {
        matches!(self, Self::Pushes)
    }
}

/// What kind of tracing style this is.
///
/// This affects things like error messages.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum TraceStyle {
    /// Parity style tracer
    Parity,
    /// Geth style tracer
    #[allow(unused)]
    Geth,
}

impl TraceStyle {
    /// Returns true if this is a parity style tracer.
    pub(crate) const fn is_parity(self) -> bool {
        matches!(self, Self::Parity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parity_config() {
        let mut s = HashSet::new();
        s.insert(TraceType::StateDiff);
        let config = TracingInspectorConfig::from_parity_config(&s);
        // not required
        assert!(!config.record_steps);
        assert!(!config.record_state_diff);

        let mut s = HashSet::new();
        s.insert(TraceType::VmTrace);
        let config = TracingInspectorConfig::from_parity_config(&s);
        assert!(config.record_steps);
        assert!(!config.record_state_diff);

        let mut s = HashSet::new();
        s.insert(TraceType::VmTrace);
        s.insert(TraceType::StateDiff);
        let config = TracingInspectorConfig::from_parity_config(&s);
        assert!(config.record_steps);
        // not required for StateDiff
        assert!(!config.record_state_diff);
    }
}
