#![allow(unused)]
use alloc::sync::Arc;

#[derive(Debug, Clone)]
pub struct ArbBlockAssembler<ChainSpec>(core::marker::PhantomData<ChainSpec>);

impl<ChainSpec> ArbBlockAssembler<ChainSpec> {
    pub fn new(_chain_spec: Arc<ChainSpec>) -> Self {
        Self(core::marker::PhantomData)
    }
}
