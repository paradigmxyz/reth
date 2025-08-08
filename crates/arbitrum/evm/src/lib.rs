#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::sync::Arc;

mod config;
pub use config::ArbBlockAssembler;

mod build;
pub use build::ArbBlockExecutorFactory;

pub mod receipts;
pub use receipts::*;

#[derive(Debug)]
pub struct ArbEvmConfig<ChainSpec = (), N = (), R = ArbRethReceiptBuilder> {
    pub executor_factory: ArbBlockExecutorFactory<R, ChainSpec>,
    pub block_assembler: ArbBlockAssembler<ChainSpec>,
    _pd: core::marker::PhantomData<N>,
}

impl<ChainSpec, N: Clone, R: Clone> Clone for ArbEvmConfig<ChainSpec, N, R> {
    fn clone(&self) -> Self {
        Self {
            executor_factory: self.executor_factory.clone(),
            block_assembler: self.block_assembler.clone(),
            _pd: self._pd.clone(),
        }
    }
}

impl<ChainSpec, N, R> Default for ArbEvmConfig<ChainSpec, N, R>
where
    ChainSpec: Default,
    R: Default + Clone,
{
    fn default() -> Self {
        let cs = Arc::new(ChainSpec::default());
        Self::new(cs, R::default())
    }
}

impl<ChainSpec, N, R> ArbEvmConfig<ChainSpec, N, R>
where
    R: Clone,
{
    pub fn new(chain_spec: Arc<ChainSpec>, receipt_builder: R) -> Self {
        Self {
            block_assembler: ArbBlockAssembler::new(chain_spec.clone()),
            executor_factory: ArbBlockExecutorFactory::new(receipt_builder, chain_spec),
            _pd: core::marker::PhantomData,
        }
    }

    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }
}
