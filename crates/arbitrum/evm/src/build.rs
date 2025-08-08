#![allow(unused)]
use alloc::sync::Arc;

#[derive(Debug, Clone)]
pub struct ArbBlockExecutorFactory<R, CS> {
    receipt_builder: R,
    spec: Arc<CS>,
}

impl<R: Clone, CS> ArbBlockExecutorFactory<R, CS> {
    pub fn new(receipt_builder: R, spec: Arc<CS>) -> Self {
        Self { receipt_builder, spec }
    }

    pub const fn spec(&self) -> &Arc<CS> {
        &self.spec
    }
}
