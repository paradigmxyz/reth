// Stubbed big-block EVM configuration.
//
// Big-block execution is parked while it is ported to evm2. This wrapper keeps the node type and
// payload shape available without retaining a direct legacy executor dependency in `reth-bb`.

pub(crate) use reth_engine_primitives::BigBlockData;

use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, Bytes};
use alloy_rpc_types::engine::ExecutionData;
use core::{convert::Infallible, fmt};
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{
    ConfigureEngineEvm, ConfigureEvm, ConfigureEvm2Prewarm, EvmEnvFor, ExecutableTxIterator,
    ExecutionCtxFor, NextBlockEnvAttributes, TxEnvFor,
};
use reth_evm_ethereum::{EthEvmConfig, EthEvmEnv, Evm2TxEnv};
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader, TxTy};
use reth_storage_api::StateProvider;
use reth_storage_errors::any::AnyError;

/// EVM configuration for big-block execution.
pub struct BbEvmConfig<C = reth_chainspec::ChainSpec> {
    inner: EthEvmConfig<C>,
}

impl<C> Clone for BbEvmConfig<C>
where
    EthEvmConfig<C>: Clone,
{
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<C> fmt::Debug for BbEvmConfig<C>
where
    EthEvmConfig<C>: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BbEvmConfig").field("inner", &self.inner).finish()
    }
}

impl<C> BbEvmConfig<C> {
    /// Creates a new stubbed big-block EVM configuration.
    pub const fn new(inner: EthEvmConfig<C>) -> Self {
        Self { inner }
    }
}

impl<C> ConfigureEvm for BbEvmConfig<C>
where
    EthEvmConfig<C>: ConfigureEvm<
        Primitives = EthPrimitives,
        Error = Infallible,
        NextBlockEnvCtx = NextBlockEnvAttributes,
        EvmEnv = EthEvmEnv,
        TxEnv = Evm2TxEnv,
    >,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type Spec = <EthEvmConfig<C> as ConfigureEvm>::Spec;
    type EvmEnv = EthEvmEnv;
    type TxEnv = Evm2TxEnv;
    type ExecutionCtx<'a>
        = <EthEvmConfig<C> as ConfigureEvm>::ExecutionCtx<'a>
    where
        Self: 'a;
    type Executor<DB>
        = <EthEvmConfig<C> as ConfigureEvm>::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block<'a>(
        &'a self,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn deposit_contract_address(&self) -> Option<Address> {
        self.inner.deposit_contract_address()
    }

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner.executor(db)
    }
}

impl<C> ConfigureEngineEvm<BigBlockData<ExecutionData>> for BbEvmConfig<C>
where
    Self: ConfigureEvm<Primitives = EthPrimitives, Error = Infallible>,
    EthEvmConfig<C>: ConfigureEngineEvm<ExecutionData>
        + ConfigureEvm<Primitives = EthPrimitives, Error = Infallible>,
{
    fn evm_env_for_payload(
        &self,
        _payload: &BigBlockData<ExecutionData>,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        unreachable!("big-block payload execution is parked while evm2 support lands")
    }

    fn context_for_payload<'a>(
        &self,
        _payload: &'a BigBlockData<ExecutionData>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        unreachable!("big-block payload execution is parked while evm2 support lands")
    }

    fn tx_iterator_for_payload(
        &self,
        _payload: &BigBlockData<ExecutionData>,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let transactions: Vec<Bytes> = Vec::new();
        let convert = |_tx: Bytes| -> Result<Recovered<TxTy<Self::Primitives>>, AnyError> {
            unreachable!("big-block payload execution is parked while evm2 support lands")
        };

        Ok((transactions, convert))
    }
}

impl<C> ConfigureEvm2Prewarm for BbEvmConfig<C>
where
    Self: ConfigureEvm<Primitives = EthPrimitives, EvmEnv = EthEvmEnv, TxEnv = Evm2TxEnv>,
    EthEvmConfig<C>: ConfigureEvm<Primitives = EthPrimitives, EvmEnv = EthEvmEnv, TxEnv = Evm2TxEnv>
        + ConfigureEvm2Prewarm<Primitives = EthPrimitives>,
{
    type PrewarmEvm<DB>
        = <EthEvmConfig<C> as ConfigureEvm2Prewarm>::PrewarmEvm<DB>
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
        self.inner.evm2_prewarm_evm_with_precompiles(state_provider, env, precompiles)
    }

    fn evm2_prewarm_tx<DB, S>(
        &self,
        evm: &mut Self::PrewarmEvm<DB>,
        tx: TxEnvFor<Self>,
        sink: &mut S,
    ) -> Result<evm2::TxResult, Box<dyn core::error::Error + Send + Sync>>
    where
        DB: StateProvider + Send + 'static,
        S: evm2::evm::StateChangeSink<Error = Infallible>,
    {
        self.inner.evm2_prewarm_tx(evm, tx, sink)
    }
}
