use crate::{CallRequest,state::StateOverride,BlockOverrides};
use std::{collections::HashSet};
use reth_primitives::{BlockId};
use reth_rpc::{types::tracerequest::parity::TraceType};
    
pub struct TraceRequest{
    call: CallRequest,
    trace_types: HashSet<TraceType>,
    block_id: Option<BlockId>,
    state_overrides: Option<StateOverride>,
    block_overrides: Option<Box<BlockOverrides>>
}

impl TraceRequest{

    pub fn new(call:CallRequest,trace_types:HashSet<TraceType>) -> Self{

        Self { call,trace_types,block_id:None, state_overrides: None, block_overrides:None }

    }

    pub fn with_block_id(mut self, block_id: BlockId) -> Self{
        self.block_id = Some(block_id);
        self
    }

    pub fn with_state_override(mut self, state_overrides:StateOverride) -> Self{
        self.state_overrides = Some(state_overrides);
        self
    }

    pub fn with_block_overrides(mut self, block_overrides:Box<BlockOverrides>) -> Self{
        self.block_overrides = Some(block_overrides);
        self
    }

}