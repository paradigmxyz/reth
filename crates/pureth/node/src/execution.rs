use crate::ExecutionEngineValidator;
use alloy_consensus::BlockHeader;
use alloy_eip7928::bal::Bal;
use alloy_evm::eth::EthBlockExecutionCtx;
use alloy_primitives::{Sealable, B256};
use alloy_rpc_types_engine::ExecutionData;
use reth_chain_state::ExecutedBlock;
use reth_consensus::{Consensus, ConsensusError, FullConsensus, HeaderValidator, ReceiptRootBloom};
use reth_consensus_common::validation::{
    validate_block_pre_execution, validate_body_against_header, validate_cancun_gas,
    MAX_RLP_BLOCK_SIZE,
};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_ethereum_primitives::Receipt;
use reth_evm::{
    execute::{BlockAssembler, BlockAssemblerInput, BlockExecutionError},
    ConfigureEngineEvm, ConfigureEvm, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor, JitBackend,
    NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::{BlockExecutionResult, RetainedExecutionData};
use reth_primitives_traits::{Block, BlockBody, RecoveredBlock, SealedBlock, SealedHeader};
use reth_pureth_block::{
    ExecutionBlock, ExecutionBody, ExecutionHeader, ExecutionPrimitives, ExecutionSource,
    PayloadSourceError, PayloadSourceInput, SourcePayloadSnapshot,
};
use std::{
    borrow::Cow,
    sync::{atomic::AtomicBool, Arc},
};

#[derive(Clone, Debug)]
pub struct ExecutionConsensus {
    validator: ExecutionEngineValidator,
}

impl ExecutionConsensus {
    pub const fn new(validator: ExecutionEngineValidator) -> Self {
        Self { validator }
    }

    fn check_mode(&self, header: &ExecutionHeader) -> Result<(), ConsensusError> {
        if header.is_ssz() !=
            self.validator.fork_config().is_active_at_timestamp(header.timestamp())
        {
            return Err(ConsensusError::other(PayloadSourceError::UnsupportedFork));
        }
        Ok(())
    }
}

impl HeaderValidator<ExecutionHeader> for ExecutionConsensus {
    fn validate_header(
        &self,
        header: &SealedHeader<ExecutionHeader>,
    ) -> Result<(), ConsensusError> {
        self.check_mode(header)?;
        if header.hash() != header.hash_slow() {
            return Err(ConsensusError::other(PayloadSourceError::SourceMismatch));
        }
        EthBeaconConsensus::new(self.validator.chain_spec().clone())
            .validate_header(&SealedHeader::new(header.as_native().clone(), header.hash()))
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<ExecutionHeader>,
        parent: &SealedHeader<ExecutionHeader>,
    ) -> Result<(), ConsensusError> {
        self.check_mode(header)?;
        self.check_mode(parent)?;
        if header.hash() != header.hash_slow() || parent.hash() != parent.hash_slow() {
            return Err(ConsensusError::other(PayloadSourceError::SourceMismatch));
        }
        EthBeaconConsensus::new(self.validator.chain_spec().clone()).validate_header_against_parent(
            &SealedHeader::new(header.as_native().clone(), header.hash()),
            &SealedHeader::new(parent.as_native().clone(), parent.hash()),
        )
    }
}

impl Consensus<ExecutionBlock> for ExecutionConsensus {
    fn validate_body_against_header(
        &self,
        body: &ExecutionBody,
        header: &SealedHeader<ExecutionHeader>,
    ) -> Result<(), ConsensusError> {
        self.check_mode(header)?;
        if body.is_ssz() != header.is_ssz() {
            return Err(ConsensusError::other(PayloadSourceError::SourceMismatch));
        }
        if header.is_ssz() {
            if header.transactions_root() != body.calculate_tx_root() ||
                header.withdrawals_root() != body.calculate_withdrawals_root()
            {
                return Err(ConsensusError::other(PayloadSourceError::SourceMismatch));
            }
            return Ok(());
        }
        validate_body_against_header(body, header.header())
    }

    fn validate_block_pre_execution(
        &self,
        block: &SealedBlock<ExecutionBlock>,
    ) -> Result<(), ConsensusError> {
        self.validate_body_against_header(block.body(), block.sealed_header())?;
        if block.header().is_ssz() {
            validate_cancun_gas(block)?;
            if block.rlp_length() > MAX_RLP_BLOCK_SIZE {
                return Err(ConsensusError::BlockTooLarge {
                    rlp_length: block.rlp_length(),
                    max_rlp_length: MAX_RLP_BLOCK_SIZE,
                });
            }
            return Ok(());
        }
        validate_block_pre_execution(block, self.validator.chain_spec().as_ref())
    }
}

impl FullConsensus<ExecutionPrimitives> for ExecutionConsensus {
    fn requires_state_root_validation(&self, timestamp: u64) -> bool {
        self.validator.fork_config().is_active_at_timestamp(timestamp)
    }

    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<ExecutionBlock>,
        result: &BlockExecutionResult<Receipt>,
        receipts: Option<ReceiptRootBloom>,
        bal_hash: Option<B256>,
    ) -> Result<(), ConsensusError> {
        self.check_mode(block.header())?;
        if block.header().is_ssz() {
            return Err(ConsensusError::other(PayloadSourceError::MissingInput(
                PayloadSourceInput::BlockAccessList,
            )));
        }
        validate_block_post_execution(
            block,
            self.validator.chain_spec().as_ref(),
            result,
            receipts,
            bal_hash,
        )
    }

    fn validate_block_post_execution_with_data(
        &self,
        block: &Arc<RecoveredBlock<ExecutionBlock>>,
        result: &BlockExecutionResult<Receipt>,
        receipts: Option<ReceiptRootBloom>,
        bal_hash: Option<B256>,
        bal: Option<&Bal>,
    ) -> Result<Option<RetainedExecutionData>, ConsensusError> {
        if !block.header().is_ssz() {
            self.validate_block_post_execution(block, result, receipts, bal_hash)?;
            return Ok(None);
        }
        self.check_mode(block.header())?;
        let bal = bal.ok_or_else(|| {
            ConsensusError::other(PayloadSourceError::MissingInput(
                PayloadSourceInput::BlockAccessList,
            ))
        })?;
        let source = ExecutionSource::from_execution(
            block.clone(),
            result,
            bal,
            self.validator.chain_spec().as_ref(),
            &self.validator.fork_config(),
            self.validator.limits(),
        )
        .map_err(|error| {
            if matches!(
                error,
                PayloadSourceError::LimitExceeded |
                    PayloadSourceError::Cancelled |
                    PayloadSourceError::Payload(reth_pureth_block::PayloadError::LimitExceeded(
                        _
                    ))
            ) {
                ConsensusError::Unavailable(Arc::new(error))
            } else {
                ConsensusError::other(error)
            }
        })?;
        Ok(Some(Arc::new(source)))
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionEvmConfig {
    inner: EthEvmConfig,
    validator: ExecutionEngineValidator,
}

impl ExecutionEvmConfig {
    pub fn new(
        inner: EthEvmConfig,
        validator: ExecutionEngineValidator,
    ) -> Result<Self, PayloadSourceError> {
        if !Arc::ptr_eq(inner.chain_spec(), validator.chain_spec()) {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        Ok(Self { inner, validator })
    }

    pub(crate) const fn native_config(&self) -> &EthEvmConfig {
        &self.inner
    }

    const fn configure_flags(&self, env: &mut EvmEnvFor<Self>, timestamp: u64) {
        let active = self.validator.fork_config().is_active_at_timestamp(timestamp);
        env.cfg_env.enable_amsterdam_eip8037 = active;
        env.cfg_env.enable_amsterdam_eip2780 = active;
    }
}

impl ConfigureEvm for ExecutionEvmConfig {
    type Primitives = ExecutionPrimitives;
    type Error = PayloadSourceError;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = <EthEvmConfig as ConfigureEvm>::BlockExecutorFactory;
    type BlockAssembler = Self;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.inner.block_executor_factory()
    }
    fn block_assembler(&self) -> &Self::BlockAssembler {
        self
    }

    fn evm_env(&self, header: &ExecutionHeader) -> Result<EvmEnvFor<Self>, Self::Error> {
        if header.is_ssz() !=
            self.validator.fork_config().is_active_at_timestamp(header.timestamp())
        {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        let mut env = self.inner.evm_env(header.as_native()).unwrap();
        self.configure_flags(&mut env, header.timestamp());
        Ok(env)
    }

    fn next_evm_env(
        &self,
        parent: &ExecutionHeader,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let mut env = self.inner.next_evm_env(parent.as_native(), attributes).unwrap();
        self.configure_flags(&mut env, attributes.timestamp);
        Ok(env)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<ExecutionBlock>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        if block.body().ommers().is_some_and(|ommers| !ommers.is_empty()) {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        Ok(EthBlockExecutionCtx {
            tx_count_hint: Some(block.transaction_count()),
            parent_hash: block.parent_hash(),
            parent_beacon_block_root: block.parent_beacon_block_root(),
            ommers: &[],
            withdrawals: block.body().withdrawals().map(|values| Cow::Borrowed(values.as_slice())),
            extra_data: block.extra_data().clone(),
            slot_number: block.slot_number(),
        })
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<ExecutionHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: None,
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            ommers: &[],
            withdrawals: attributes.withdrawals.map(|values| Cow::Owned(values.into_inner())),
            extra_data: attributes.extra_data,
            slot_number: attributes.slot_number,
        })
    }

    fn with_jit_support_enabled(mut self, enabled: bool) -> Self {
        self.inner = self.inner.with_jit_support_enabled(enabled);
        self
    }
    fn jit_backend(&self) -> Option<&dyn JitBackend> {
        self.inner.jit_backend()
    }
}

impl ConfigureEngineEvm<ExecutionData> for ExecutionEvmConfig {
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        let mut env = self.inner.evm_env_for_payload(payload).unwrap();
        self.configure_flags(&mut env, payload.payload.timestamp());
        Ok(env)
    }
    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        Ok(self.inner.context_for_payload(payload).unwrap())
    }
    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        Ok(self.inner.tx_iterator_for_payload(payload).unwrap())
    }
}

impl BlockAssembler<<Self as ConfigureEvm>::BlockExecutorFactory> for ExecutionEvmConfig {
    type Block = ExecutionBlock;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<
            '_,
            '_,
            <Self as ConfigureEvm>::BlockExecutorFactory,
            ExecutionHeader,
        >,
    ) -> Result<ExecutionBlock, BlockExecutionError> {
        let parent = SealedHeader::new(input.parent.as_native().clone(), input.parent.hash());
        let result = input.output;
        let bal = input.block_access_list;
        let mut native_input =
            BlockAssemblerInput::<<Self as ConfigureEvm>::BlockExecutorFactory>::new(
                input.evm_env,
                input.execution_ctx,
                &parent,
                input.transactions,
                result,
                input.bundle_state,
                input.state_provider,
                input.state_root,
                input.block_access_list_hash,
            );
        native_input.block_access_list = bal;
        let native = BlockAssembler::assemble_block(self.inner.block_assembler(), native_input)?;
        if !self.validator.fork_config().is_active_at_timestamp(native.timestamp()) {
            let header = ExecutionHeader::legacy(native.header);
            let body = ExecutionBody::from_header(
                &header,
                native.body.map_ommers(ExecutionHeader::legacy),
            )
            .map_err(BlockExecutionError::other)?;
            return ExecutionBlock::try_new(header, body).map_err(BlockExecutionError::other);
        }
        let bal = bal.ok_or_else(|| {
            BlockExecutionError::other(PayloadSourceError::MissingInput(
                PayloadSourceInput::BlockAccessList,
            ))
        })?;
        let recovered = native.try_into_recovered().map_err(BlockExecutionError::other)?;
        let snapshot = SourcePayloadSnapshot::from_block_execution(
            &recovered,
            result,
            bal.as_raw().clone(),
            self.validator.chain_spec().as_ref(),
            self.validator.limits(),
        )
        .map_err(BlockExecutionError::other)?;
        let source = snapshot
            .into_execution_source(
                recovered,
                result.receipts.clone(),
                self.validator.chain_spec().as_ref(),
                &self.validator.fork_config(),
                &AtomicBool::new(false),
            )
            .map_err(BlockExecutionError::other)?;
        Ok(source.block().clone().into_sealed_block().into_block())
    }
}

pub fn received_source(
    block: &ExecutedBlock<ExecutionPrimitives>,
) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
    let source = block
        .retained_execution_data
        .as_ref()
        .and_then(|data| data.clone().downcast::<ExecutionSource>().ok())
        .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::FullPayload))?;
    if !Arc::ptr_eq(source.block_arc(), &block.recovered_block) {
        return Err(PayloadSourceError::SourceMismatch);
    }
    Ok(source)
}
