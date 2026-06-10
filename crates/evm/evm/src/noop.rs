//! Helpers for testing.

use crate::{ConfigureEvm, EvmEnvFor};
#[cfg(feature = "std")]
use crate::{ConfigureEvm2BlockExecutor, ConfigureEvm2Prewarm};
#[cfg(feature = "std")]
use alloc::boxed::Box;
#[cfg(feature = "std")]
use reth_execution_types::BlockExecutionOutput;
#[cfg(feature = "std")]
use reth_primitives_traits::TxTy;
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader};
#[cfg(feature = "std")]
use reth_primitives_traits::{ReceiptTy, RecoveredBlock};
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
}

#[cfg(feature = "std")]
impl<Inner> ConfigureEvm2BlockExecutor for NoopEvmConfig<Inner>
where
    Inner: ConfigureEvm2BlockExecutor,
{
    type Primitives = Inner::Primitives;

    fn execute_evm2_block_with_state_provider<DB>(
        &self,
        state_provider: DB,
        block: &RecoveredBlock<BlockTy<<Self as ConfigureEvm2BlockExecutor>::Primitives>>,
    ) -> Result<
        BlockExecutionOutput<ReceiptTy<<Self as ConfigureEvm2BlockExecutor>::Primitives>>,
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
        block: &RecoveredBlock<BlockTy<<Self as ConfigureEvm2BlockExecutor>::Primitives>>,
    ) -> Result<
        BlockExecutionOutput<ReceiptTy<<Self as ConfigureEvm2BlockExecutor>::Primitives>>,
        Box<dyn core::error::Error + Send + Sync>,
    > {
        self.inner().execute_evm2_block_with_state_provider_ref(state_provider, block)
    }

    fn execute_evm2_block_with_database<DB>(
        &self,
        database: DB,
        block: &RecoveredBlock<BlockTy<<Self as ConfigureEvm2BlockExecutor>::Primitives>>,
    ) -> Result<
        BlockExecutionOutput<ReceiptTy<<Self as ConfigureEvm2BlockExecutor>::Primitives>>,
        Box<dyn core::error::Error + Send + Sync>,
    >
    where
        DB: evm2::evm::Database + 'static,
        DB::Error: Send + Sync,
    {
        self.inner().execute_evm2_block_with_database(database, block)
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

    fn evm2_prewarm_evm<DB>(&self, state_provider: DB, env: EvmEnvFor<Self>) -> Self::PrewarmEvm<DB>
    where
        DB: StateProvider + Send + 'static,
    {
        self.inner().evm2_prewarm_evm(state_provider, env)
    }

    fn evm2_prewarm_spec(&self, env: &EvmEnvFor<Self>) -> evm2::SpecId {
        self.inner().evm2_prewarm_spec(env)
    }

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

    fn evm2_prewarm_tx<DB>(
        &self,
        evm: &mut Self::PrewarmEvm<DB>,
        tx: crate::TxEnvFor<Self>,
    ) -> Result<evm2::TxResultWithState, Box<dyn core::error::Error + Send + Sync>>
    where
        DB: StateProvider + Send + 'static,
    {
        self.inner().evm2_prewarm_tx(evm, tx)
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
    fn evm2_chain_id(&self) -> u64 {
        self.inner().evm2_chain_id()
    }

    fn evm2_deposit_contract_address(&self) -> Option<alloy_primitives::Address> {
        self.inner().evm2_deposit_contract_address()
    }

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
