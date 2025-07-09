use alloc::sync::Arc;
use alloy_consensus::{proofs, BlockBody, Header, TxReceipt, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::merge::BEACON_NONCE;
use alloy_evm::block::{BlockExecutionError, BlockExecutorFactory};
use alloy_primitives::{logs_bloom, Address};
use reth_evm::execute::{BlockAssembler, BlockAssemblerInput};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::SignedTransaction;
use reth_scroll_primitives::ScrollReceipt;
use scroll_alloy_evm::ScrollBlockExecutionCtx;
use scroll_alloy_hardforks::ScrollHardforks;

/// Block builder for Scroll.
#[derive(Debug)]
pub struct ScrollBlockAssembler<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> ScrollBlockAssembler<ChainSpec> {
    /// Creates a new [`ScrollBlockAssembler`].
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<ChainSpec> Clone for ScrollBlockAssembler<ChainSpec> {
    fn clone(&self) -> Self {
        Self { chain_spec: self.chain_spec.clone() }
    }
}

impl<F, ChainSpec> BlockAssembler<F> for ScrollBlockAssembler<ChainSpec>
where
    ChainSpec: ScrollHardforks,
    F: for<'a> BlockExecutorFactory<
        ExecutionCtx<'a> = ScrollBlockExecutionCtx,
        Transaction: SignedTransaction,
        Receipt = ScrollReceipt,
    >,
{
    type Block = alloy_consensus::Block<F::Transaction>;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F>,
    ) -> Result<Self::Block, BlockExecutionError> {
        let BlockAssemblerInput {
            evm_env,
            execution_ctx: ctx,
            transactions,
            output: BlockExecutionResult { receipts, gas_used, .. },
            state_root,
            ..
        } = input;

        let timestamp = evm_env.block_env.timestamp;

        let transactions_root = proofs::calculate_transaction_root(&transactions);
        let receipts_root = ScrollReceipt::calculate_receipt_root_no_memo(receipts);
        let logs_bloom = logs_bloom(receipts.iter().flat_map(|r| r.logs()));

        let header = Header {
            parent_hash: ctx.parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Address::ZERO,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: None,
            logs_bloom,
            timestamp: timestamp.to(),
            mix_hash: evm_env.block_env.prevrandao.unwrap_or_default(),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: self
                .chain_spec
                .is_curie_active_at_block(evm_env.block_env.number.to())
                .then_some(evm_env.block_env.basefee),
            number: evm_env.block_env.number.to(),
            gas_limit: evm_env.block_env.gas_limit,
            difficulty: evm_env.block_env.difficulty,
            gas_used: *gas_used,
            extra_data: Default::default(),
            parent_beacon_block_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            requests_hash: None,
        };

        Ok(alloy_consensus::Block::new(
            header,
            BlockBody { transactions, ommers: Default::default(), withdrawals: None },
        ))
    }
}
