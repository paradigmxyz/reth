//! EVM configuration for merged big-block payloads.

pub(crate) use reth_engine_primitives::BigBlockData;

use alloy_consensus::Header;
use alloy_eips::Decodable2718;
use alloy_primitives::Bytes;
use alloy_rpc_types::engine::ExecutionData;
use core::{convert::Infallible, fmt};
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives, TransactionSigned};
use reth_evm::{
    BlockAssembler, BlockAssemblerInput, BlockExecutionError, ConfigureEngineEvm, ConfigureEvm,
    DynDatabase, EvmEnvFor, EvmState, ExecutableTxIterator, ExecutionCtxFor,
    NextBlockEnvAttributes,
};
use reth_evm_ethereum::{
    EthBigBlockExecutorFactory, EthBigBlockPlan, EthBigBlockSegment, EthEvmConfig,
    ExecutableRecoveredTx,
};
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader, SignedTransaction};
use reth_storage_errors::any::AnyError;
use std::vec::Vec;

/// Block assembler marker for replay-only big-block execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct BbBlockAssembler;

impl<C> BlockAssembler<EthBigBlockExecutorFactory<C>> for BbBlockAssembler
where
    C: EthChainSpec<Header = Header> + EthereumHardforks + 'static,
{
    type Block = Block;

    fn assemble_block(
        &self,
        _input: BlockAssemblerInput<'_, '_, EthBigBlockExecutorFactory<C>, Header>,
    ) -> Result<Self::Block, BlockExecutionError> {
        unreachable!("big-block execution is replay-only")
    }
}

/// EVM configuration for big-block execution.
pub struct BbEvmConfig<C = ChainSpec> {
    inner: EthEvmConfig<C>,
    executor_factory: EthBigBlockExecutorFactory<C>,
    block_assembler: BbBlockAssembler,
}

impl<C> Clone for BbEvmConfig<C>
where
    EthEvmConfig<C>: Clone,
    EthBigBlockExecutorFactory<C>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            executor_factory: self.executor_factory.clone(),
            block_assembler: self.block_assembler,
        }
    }
}

impl<C> fmt::Debug for BbEvmConfig<C>
where
    EthEvmConfig<C>: fmt::Debug,
    EthBigBlockExecutorFactory<C>: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BbEvmConfig")
            .field("inner", &self.inner)
            .field("executor_factory", &self.executor_factory)
            .finish()
    }
}

impl<C> BbEvmConfig<C> {
    /// Creates a big-block EVM configuration from the standard Ethereum configuration.
    pub fn new(inner: EthEvmConfig<C>) -> Self
    where
        EthEvmConfig<C>: Clone,
        EthBigBlockExecutorFactory<C>: Clone,
    {
        let executor_factory = EthBigBlockExecutorFactory::new(inner.executor_factory.clone());
        Self { inner, executor_factory, block_assembler: BbBlockAssembler }
    }
}

impl<C> ConfigureEvm for BbEvmConfig<C>
where
    C: EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = EthBigBlockExecutorFactory<C>;
    type BlockAssembler = BbBlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
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
        let evm_env = self.inner.evm_env(block.header())?;
        let ctx = self.inner.context_for_block(block)?;
        Ok(EthBigBlockPlan::new(
            vec![EthBigBlockSegment { start_tx: 0, evm_env, ctx }],
            Vec::new(),
            block.transaction_count(),
        ))
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error> {
        let evm_env = self.inner.next_evm_env(parent, &attributes)?;
        let ctx = self.inner.context_for_next_block(parent, attributes)?;
        Ok(EthBigBlockPlan::new(
            vec![EthBigBlockSegment { start_tx: 0, evm_env, ctx }],
            Vec::new(),
            0,
        ))
    }

    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        block_number: u64,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> Result<EvmState, Box<dyn core::error::Error + Send + Sync>>
    where
        Self: 'a,
        DB: DynDatabase + 'a,
    {
        let segment = ctx.segments.first().expect("big-block context has a segment");
        self.inner.pre_block_state_changes(db, evm_env, block_number, segment.ctx.clone())
    }
}

impl<C> ConfigureEngineEvm<BigBlockData<ExecutionData>> for BbEvmConfig<C>
where
    C: EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
{
    fn evm_env_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let first = payload.env_switches.first().expect("big-block payload has no segments");
        self.inner.evm_env_for_payload(first)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a BigBlockData<ExecutionData>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        assert!(!payload.env_switches.is_empty(), "big-block payload has no segments");

        let mut start_tx = 0;
        let mut segments = Vec::with_capacity(payload.env_switches.len());
        for data in &payload.env_switches {
            let evm_env = self.inner.evm_env_for_payload(data)?;
            let ctx = self.inner.context_for_payload(data)?;
            segments.push(EthBigBlockSegment { start_tx, evm_env, ctx });
            start_tx += data.payload.transactions().len();
        }

        Ok(EthBigBlockPlan::new(segments, payload.prior_block_hashes.clone(), start_tx))
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let transactions = payload
            .env_switches
            .iter()
            .flat_map(|data| data.payload.transactions().iter().cloned())
            .collect::<Vec<_>>();
        let convert = |tx: Bytes| {
            let tx = TransactionSigned::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
            let signer = tx.try_recover().map_err(AnyError::new)?;
            Ok::<_, AnyError>(ExecutableRecoveredTx::new(tx.with_signer(signer)))
        };

        Ok((transactions, convert))
    }
}
