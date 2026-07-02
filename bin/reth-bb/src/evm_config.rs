// Stubbed big-block EVM configuration.
//
// Big-block execution is parked while it is ported to the active EVM. This wrapper keeps the node
// type and payload shape available without retaining a direct legacy executor dependency in
// `reth-bb`.

pub(crate) use reth_engine_primitives::BigBlockData;

use alloy_consensus::transaction::Recovered;
use alloy_primitives::Bytes;
use alloy_rpc_types::engine::ExecutionData;
use core::{convert::Infallible, fmt};
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{
    ConfigureEngineEvm, ConfigureEvm, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor,
    NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader, TxTy};
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
    >,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = <EthEvmConfig<C> as ConfigureEvm>::BlockExecutorFactory;
    type BlockAssembler = <EthEvmConfig<C> as ConfigureEvm>::BlockAssembler;

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
