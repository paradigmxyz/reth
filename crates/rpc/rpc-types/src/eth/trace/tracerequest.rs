//! Builder style functions for `trace_call`

use crate::{state::StateOverride, trace::parity::TraceType, BlockOverrides, CallRequest};
use reth_primitives::BlockId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Trace Request builder style function implementation
#[derive(Debug, Serialize, Deserialize)]
pub struct TraceRequest {
    /// call request object
    pub call: CallRequest,
    /// trace types
    pub trace_types: HashSet<TraceType>,
    /// Optional: blockId
    pub block_id: Option<BlockId>,
    /// Optional: StateOverride
    pub state_overrides: Option<StateOverride>,
    /// Optional: BlockOverrides
    pub block_overrides: Option<Box<BlockOverrides>>,
}

impl TraceRequest {
    /// Returns a new [`TraceRequest`] given a [`CallRequest`] and [`HashSet<TraceType>`]
    pub fn new(call: CallRequest) -> Self {
        Self {
            call,
            trace_types: HashSet::new(),
            block_id: None,
            state_overrides: None,
            block_overrides: None,
        }
    }

    /// Sets the [`BlockId`]
    /// Note: this is optional
    pub fn with_block_id(mut self, block_id: BlockId) -> Self {
        self.block_id = Some(block_id);
        self
    }

    /// Sets the [`StateOverride`]
    /// Note: this is optional
    pub fn with_state_override(mut self, state_overrides: StateOverride) -> Self {
        self.state_overrides = Some(state_overrides);
        self
    }

    /// Sets the [`BlockOverrides`]
    /// Note: this is optional
    pub fn with_block_overrides(mut self, block_overrides: Box<BlockOverrides>) -> Self {
        self.block_overrides = Some(block_overrides);
        self
    }

    /// Inserts a single trace type.
    pub fn with_trace_type(mut self, trace_type: TraceType) -> Self {
        self.trace_types.insert(trace_type);
        self
    }

    /// Inserts multiple trace types from an iterator.
    pub fn with_trace_types<I: IntoIterator<Item = TraceType>>(mut self, trace_types: I) -> Self {
        self.trace_types.extend(trace_types);
        self
    }

    /// Inserts [`TraceType::Trace`]
    pub fn with_trace(self) -> Self {
        self.with_trace_type(TraceType::Trace)
    }

    /// Inserts [`TraceType::VmTrace`]
    pub fn with_vm_trace(self) -> Self {
        self.with_trace_type(TraceType::VmTrace)
    }

    /// Inserts [`TraceType::StateDiff`]
    pub fn with_statediff(self) -> Self {
        self.with_trace_type(TraceType::StateDiff)
    }
}
