// Stubbed big-block EVM configuration.
//
// Big-block execution is parked while it is ported to evm2. This wrapper keeps the node type and
// payload shape available without retaining a direct legacy executor dependency in `reth-bb`.

pub(crate) use reth_engine_primitives::BigBlockData;

use alloy_consensus::transaction::Recovered;
use alloy_eips::Decodable2718;
use alloy_primitives::Bytes;
use alloy_rpc_types::engine::ExecutionData;
use core::{convert::Infallible, fmt};
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{
    execute::BlockExecutionOutput, ConfigureEngineEvm, ConfigureEvm, ConfigureEvm2BlockExecutor,
    ConfigureEvm2Engine, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{
    BlockTy, HeaderTy, ReceiptTy, RecoveredBlock, SealedBlock, SealedHeader, SignedTransaction,
    TxTy,
};
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
    >,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type Spec = <EthEvmConfig<C> as ConfigureEvm>::Spec;
    type EvmEnv = <EthEvmConfig<C> as ConfigureEvm>::EvmEnv;
    type TxEnv = <EthEvmConfig<C> as ConfigureEvm>::TxEnv;
    type ExecutionCtx<'a>
        = <EthEvmConfig<C> as ConfigureEvm>::ExecutionCtx<'a>
    where
        Self: 'a;

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

impl<C> ConfigureEvm2Engine<BigBlockData<ExecutionData>> for BbEvmConfig<C>
where
    Self: ConfigureEngineEvm<
        BigBlockData<ExecutionData>,
        Primitives = EthPrimitives,
        Error = Infallible,
    >,
    EthEvmConfig<C>: ConfigureEvm2Engine<ExecutionData>
        + ConfigureEngineEvm<ExecutionData>
        + ConfigureEvm<Primitives = EthPrimitives, Error = Infallible>,
{
    fn evm2_spec_for_header(
        &self,
        header: &HeaderTy<Self::Primitives>,
    ) -> Result<evm2::SpecId, Self::Error> {
        self.inner.evm2_spec_for_header(header)
    }

    fn evm2_block_env_for_header(
        &self,
        header: &HeaderTy<Self::Primitives>,
    ) -> Result<evm2::env::BlockEnv, Self::Error> {
        self.inner.evm2_block_env_for_header(header)
    }

    fn evm2_spec_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<evm2::SpecId, Self::Error> {
        self.inner.evm2_spec_for_payload(&payload.env_switches[0])
    }

    fn evm2_block_env_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<evm2::env::BlockEnv, Self::Error> {
        self.inner.evm2_block_env_for_payload(&payload.env_switches[0])
    }

    fn evm2_recovered_txs_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<Vec<Recovered<TxTy<Self::Primitives>>>, Box<dyn core::error::Error + Send + Sync>>
    {
        payload
            .env_switches
            .iter()
            .flat_map(|exec_data| exec_data.payload.transactions().iter())
            .map(|tx| {
                let tx =
                    TransactionSigned::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
                let signer = tx.try_recover().map_err(AnyError::new)?;
                Ok(tx.with_signer(signer))
            })
            .collect::<Result<Vec<_>, AnyError>>()
            .map_err(|err| Box::new(err) as Box<dyn core::error::Error + Send + Sync>)
    }
}

impl<C> ConfigureEvm2BlockExecutor for BbEvmConfig<C>
where
    Self: ConfigureEvm<Primitives = EthPrimitives>,
    EthEvmConfig<C>: ConfigureEvm2BlockExecutor<Primitives = EthPrimitives>,
{
    type Primitives = EthPrimitives;

    fn execute_evm2_block_with_state_provider<DB>(
        &self,
        state_provider: DB,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionOutput<Receipt>, Box<dyn core::error::Error + Send + Sync>>
    where
        DB: StateProvider + Send + 'static,
    {
        self.inner.execute_evm2_block_with_state_provider(state_provider, block)
    }

    fn execute_evm2_block_with_state_provider_ref(
        &self,
        state_provider: &dyn StateProvider,
        block: &RecoveredBlock<Block>,
    ) -> Result<
        BlockExecutionOutput<ReceiptTy<Self::Primitives>>,
        Box<dyn core::error::Error + Send + Sync>,
    > {
        self.inner.execute_evm2_block_with_state_provider_ref(state_provider, block)
    }
}
