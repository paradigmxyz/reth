#![allow(unused)]
use alloc::sync::Arc;
use alloy_primitives::{Bytes, B256};

#[derive(Debug, Clone)]
pub struct ArbBlockExecutorFactory<R, CS> {
    receipt_builder: R,
    spec: Arc<CS>,
}

#[derive(Debug, Clone, Default)]
pub struct ArbBlockExecutionCtx {
    pub parent_hash: B256,
    pub parent_beacon_block_root: Option<B256>,
    pub extra_data: Bytes,
}

impl<R: Clone, CS> ArbBlockExecutorFactory<R, CS> {
    pub fn new(receipt_builder: R, spec: Arc<CS>) -> Self {
        Self { receipt_builder, spec }
    }

    pub const fn spec(&self) -> &Arc<CS> {
        &self.spec
    }
}
