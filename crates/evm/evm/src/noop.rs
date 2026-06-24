//! Helpers for testing.

use crate::{ConfigureEvm, EvmEnvFor};
#[cfg(feature = "std")]
use alloc::boxed::Box;
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader};

/// A no-op EVM config that panics on any call. Used as a typesystem hack to satisfy
/// [`ConfigureEvm`] bounds.
#[derive(Debug, Clone)]
pub struct NoopEvmConfig<Inner>(core::marker::PhantomData<Inner>);

impl<Inner> Default for NoopEvmConfig<Inner> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Inner> NoopEvmConfig<Inner> {
    /// Create a new instance of the no-op EVM config.
    pub const fn new() -> Self {
        Self(core::marker::PhantomData)
    }

    fn inner(&self) -> &Inner {
        unimplemented!("NoopEvmConfig should never be called")
    }
}

impl<Inner> ConfigureEvm for NoopEvmConfig<Inner>
where
    Inner: ConfigureEvm,
{
    type Primitives = Inner::Primitives;
    type Error = Inner::Error;
    type NextBlockEnvCtx = Inner::NextBlockEnvCtx;
    type EvmEnv = Inner::EvmEnv;
    type TxEnv = Inner::TxEnv;
    type ExecutionCtx<'a>
        = Inner::ExecutionCtx<'a>
    where
        Self: 'a;
    #[cfg(feature = "std")]
    type BlockExecutorFactory = Inner::BlockExecutorFactory;
    #[cfg(feature = "std")]
    type BlockAssembler = Inner::BlockAssembler;
    type Executor<DB>
        = Inner::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;
    #[cfg(feature = "std")]
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.inner().block_executor_factory()
    }

    #[cfg(feature = "std")]
    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner().block_assembler()
    }

    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner().evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner().next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<crate::ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        self.inner().context_for_block(block)
    }

    fn context_for_next_block<'a>(
        &'a self,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<crate::ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        self.inner().context_for_next_block(parent, attributes)
    }

    fn chain_id(&self) -> u64 {
        self.inner().chain_id()
    }

    fn deposit_contract_address(&self) -> Option<alloy_primitives::Address> {
        self.inner().deposit_contract_address()
    }

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner().executor(db)
    }

    #[cfg(feature = "std")]
    fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: crate::ExecutionCtxFor<'a, Self>,
        hashed_state_mode: crate::execute::HashedStateMode,
    ) -> crate::BlockExecutorFor<'a, Self, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner().create_executor(evm, ctx, hashed_state_mode)
    }

    #[cfg(feature = "std")]
    fn evm_with_env<DB>(&self, db: DB, evm_env: EvmEnvFor<Self>) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: evm2::evm::DynDatabase + 'static,
    {
        self.inner().evm_with_env(db, evm_env)
    }

    #[cfg(feature = "std")]
    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        block_number: u64,
        ctx: crate::ExecutionCtxFor<'a, Self>,
    ) -> Result<evm2::BlockStateAccumulator, Box<dyn core::error::Error + Send + Sync>>
    where
        Self: 'a,
        DB: evm2::evm::Database + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner().pre_block_state_changes(db, evm_env, block_number, ctx)
    }
}

#[cfg(feature = "std")]
impl<Inner, T> crate::ConfigureEngineEvm<T> for NoopEvmConfig<Inner>
where
    Inner: crate::ConfigureEngineEvm<T>,
{
    fn evm_env_for_payload(&self, payload: &T) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner().evm_env_for_payload(payload)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a T,
    ) -> Result<crate::ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner().context_for_payload(payload)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &T,
    ) -> Result<impl crate::ExecutableTxIterator<Self>, Self::Error> {
        self.inner().tx_iterator_for_payload(payload)
    }
}
