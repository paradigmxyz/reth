//! Helpers for testing.

#[cfg(feature = "std")]
use crate::ConfigureEvm2Prewarm;
use crate::{ConfigureEvm, EvmEnvFor};
#[cfg(feature = "std")]
use alloc::boxed::Box;
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader};
#[cfg(feature = "std")]
use reth_storage_api::StateProvider;

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
    type Spec = Inner::Spec;
    type EvmEnv = Inner::EvmEnv;
    type TxEnv = Inner::TxEnv;
    type ExecutionCtx<'a>
        = Inner::ExecutionCtx<'a>
    where
        Self: 'a;
    type Executor<DB>
        = Inner::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

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
}

#[cfg(feature = "std")]
impl<Inner> ConfigureEvm2Prewarm for NoopEvmConfig<Inner>
where
    Inner: ConfigureEvm2Prewarm,
{
    type PrewarmEvm<DB>
        = Inner::PrewarmEvm<DB>
    where
        DB: StateProvider + Send + 'static;

    fn evm2_prewarm_evm_with_precompiles<DB>(
        &self,
        state_provider: DB,
        env: EvmEnvFor<Self>,
        precompiles: Box<dyn evm2::precompile::PrecompileProvider<evm2::BaseEvmTypes>>,
    ) -> Self::PrewarmEvm<DB>
    where
        DB: StateProvider + Send + 'static,
    {
        self.inner().evm2_prewarm_evm_with_precompiles(state_provider, env, precompiles)
    }

    fn evm2_prewarm_tx<DB, S>(
        &self,
        evm: &mut Self::PrewarmEvm<DB>,
        tx: crate::TxEnvFor<Self>,
        sink: &mut S,
    ) -> Result<evm2::TxResult, Box<dyn core::error::Error + Send + Sync>>
    where
        DB: StateProvider + Send + 'static,
        S: evm2::evm::StateChangeSink<Error = core::convert::Infallible>,
    {
        self.inner().evm2_prewarm_tx(evm, tx, sink)
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
