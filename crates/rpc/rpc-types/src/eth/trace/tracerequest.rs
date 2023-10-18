//! Builder style functions for `trace_call`

use crate::{state::StateOverride, trace::parity::TraceType, BlockOverrides, CallRequest};
use reth_primitives::BlockId;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Trace Request builder style function implementation
#[derive(Debug, Serialize, Deserialize)]
pub struct TraceRequest {
    /// call request object
    pub call: CallRequest,
    /// trace types
    pub trace_types: HashSet<TraceType>,
    block_id: Option<BlockId>,
    state_overrides: Option<StateOverride>,
    block_overrides: Option<Box<BlockOverrides>>,
}

impl TraceRequest {
    /// Returns a new [`TraceRequest`] given a [`CallRequest`] and [`HashSet<TraceType>`]
    pub fn new(call: CallRequest, trace_types: HashSet<TraceType>) -> Self {
        Self { call, trace_types, block_id: None, state_overrides: None, block_overrides: None }
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

    /// Returns [`Option<BlockId>`]
    pub fn block_id(&self) -> &Option<BlockId> {
        &self.block_id
    }

    ///  Returns [`Option<StateOverride>`]
    pub fn state_overrides(&self) -> &Option<StateOverride> {
        &self.state_overrides
    }

    /// Returns [`Option<Box<BlockOverrides>>`]
    pub fn block_overrides(&self) -> &Option<Box<BlockOverrides>> {
        &self.block_overrides
    }
}
