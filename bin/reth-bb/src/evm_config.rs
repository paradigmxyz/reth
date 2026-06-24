// Stubbed big-block EVM configuration.
//
// Big-block execution is parked while it is ported to the active EVM. This wrapper keeps the node
// type and payload shape available without retaining a direct legacy executor dependency in
// `reth-bb`.

pub(crate) use reth_engine_primitives::BigBlockData;

use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, Bytes};
use alloy_rpc_types::engine::ExecutionData;
use core::{convert::Infallible, fmt};
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{
    ConfigureEngineEvm, ConfigureEvm, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor,
    NextBlockEnvAttributes,
};
use reth_evm_ethereum::{EthEvmConfig, EthEvmEnv, EthTxEnv};
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
        TxEnv = EthTxEnv,
    >,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type Spec = <EthEvmConfig<C> as ConfigureEvm>::Spec;
    type EvmEnv = EthEvmEnv;
    type TxEnv = EthTxEnv;
    type ExecutionCtx<'a>
        = <EthEvmConfig<C> as ConfigureEvm>::ExecutionCtx<'a>
    where
        Self: 'a;
    type BlockExecutorFactory = <EthEvmConfig<C> as ConfigureEvm>::BlockExecutorFactory;
    type BlockAssembler = <EthEvmConfig<C> as ConfigureEvm>::BlockAssembler;
    type Executor<DB>
        = <EthEvmConfig<C> as ConfigureEvm>::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.inner.block_executor_factory()
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

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

    fn evm_tx<'a>(&self, tx: &'a EthTxEnv) -> &'a <evm2::BaseEvmTypes as evm2::EvmTypes>::Tx {
        self.inner.evm_tx(tx)
    }

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner.executor(db)
    }

    fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: ExecutionCtxFor<'a, Self>,
        hashed_state_mode: reth_evm::execute::HashedStateMode,
    ) -> reth_evm::BlockExecutorFor<'a, Self, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner.create_executor(evm, ctx, hashed_state_mode)
    }

    fn evm_with_env<DB>(&self, db: DB, evm_env: EvmEnvFor<Self>) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: evm2::evm::DynDatabase + 'static,
    {
        self.inner.evm_with_env(db, evm_env)
    }

    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        block_number: u64,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> Result<evm2::BlockStateAccumulator, Box<dyn core::error::Error + Send + Sync>>
    where
        Self: 'a,
        DB: evm2::evm::Database + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.inner.pre_block_state_changes(db, evm_env, block_number, ctx)
    }

    fn prewarm_evm<DB>(
        &self,
        state_provider: DB,
        env: EvmEnvFor<Self>,
    ) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: StateProvider + Send + 'static,
    {
        self.inner.prewarm_evm(state_provider, env)
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
        unreachable!("big-block payload execution is unsupported by the active EVM path")
    }

    fn context_for_payload<'a>(
        &self,
        _payload: &'a BigBlockData<ExecutionData>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        unreachable!("big-block payload execution is unsupported by the active EVM path")
    }

    fn tx_iterator_for_payload(
        &self,
        _payload: &BigBlockData<ExecutionData>,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let transactions: Vec<Bytes> = Vec::new();
        let convert = |_tx: Bytes| -> Result<Recovered<TxTy<Self::Primitives>>, AnyError> {
            unreachable!("big-block payload execution is unsupported by the active EVM path")
        };

        Ok((transactions, convert))
    }
}
