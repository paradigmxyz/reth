//! Helpers for testing.

use crate::{ConfigureEvm, ConfigureEvm2BlockExecutor, EvmEnvFor};
use alloc::boxed::Box;
use reth_execution_types::BlockExecutionOutput;
use reth_primitives_traits::{
    BlockTy, HeaderTy, ReceiptTy, RecoveredBlock, SealedBlock, SealedHeader, TxTy,
};
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
    type BlockExecutorFactory = Inner::BlockExecutorFactory;
    type BlockAssembler = Inner::BlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.inner().block_executor_factory()
    }

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
    ) -> Result<crate::ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner().context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<crate::ExecutionCtxFor<'_, Self>, Self::Error> {
        self.inner().context_for_next_block(parent, attributes)
    }
}

impl<Inner> ConfigureEvm2BlockExecutor for NoopEvmConfig<Inner>
where
    Inner: ConfigureEvm2BlockExecutor,
{
    fn execute_evm2_block_with_state_provider<DB>(
        &self,
        state_provider: DB,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
    ) -> Result<
        BlockExecutionOutput<ReceiptTy<Self::Primitives>>,
        Box<dyn core::error::Error + Send + Sync>,
    >
    where
        DB: StateProvider + Send + 'static,
    {
        self.inner().execute_evm2_block_with_state_provider(state_provider, block)
    }

    fn execute_evm2_block_with_state_provider_ref(
        &self,
        state_provider: &dyn StateProvider,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
    ) -> Result<
        BlockExecutionOutput<ReceiptTy<Self::Primitives>>,
        Box<dyn core::error::Error + Send + Sync>,
    > {
        self.inner().execute_evm2_block_with_state_provider_ref(state_provider, block)
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

#[cfg(feature = "std")]
impl<Inner, T> crate::ConfigureEvm2Engine<T> for NoopEvmConfig<Inner>
where
    Inner: crate::ConfigureEvm2Engine<T>,
{
    fn evm2_spec_for_header(
        &self,
        header: &HeaderTy<Self::Primitives>,
    ) -> Result<evm2::SpecId, Self::Error> {
        self.inner().evm2_spec_for_header(header)
    }

    fn evm2_block_env_for_header(
        &self,
        header: &HeaderTy<Self::Primitives>,
    ) -> Result<evm2::env::BlockEnv, Self::Error> {
        self.inner().evm2_block_env_for_header(header)
    }

    fn evm2_spec_for_payload(&self, payload: &T) -> Result<evm2::SpecId, Self::Error> {
        self.inner().evm2_spec_for_payload(payload)
    }

    fn evm2_block_env_for_payload(&self, payload: &T) -> Result<evm2::env::BlockEnv, Self::Error> {
        self.inner().evm2_block_env_for_payload(payload)
    }

    fn evm2_recovered_txs_for_payload(
        &self,
        payload: &T,
    ) -> Result<
        alloc::vec::Vec<alloy_consensus::transaction::Recovered<TxTy<Self::Primitives>>>,
        alloc::boxed::Box<dyn core::error::Error + Send + Sync>,
    > {
        self.inner().evm2_recovered_txs_for_payload(payload)
    }
}
