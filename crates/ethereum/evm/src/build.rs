use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::{
    proofs::{self, calculate_receipt_root},
    Block, BlockBody, BlockHeader, Header, TxReceipt, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::{eip4895::Withdrawals, merge::BEACON_NONCE};
use alloy_primitives::{Bloom, B256};
use reth_chainspec::{EthChainSpec, EthExecutorSpec, EthereumHardforks};
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::execute::{
    BlockAssembler, BlockAssemblerInput, BlockExecutionError, BlockExecutorFactory,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::logs_bloom as calculate_logs_bloom;

use crate::{EthBlockExecutionCtx, EthBlockExecutorFactory, EthEvmEnv};

/// Block builder for Ethereum.
#[derive(Debug)]
pub struct EthBlockAssembler<ChainSpec = reth_chainspec::ChainSpec> {
    /// The chainspec.
    pub chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> Clone for EthBlockAssembler<ChainSpec> {
    fn clone(&self) -> Self {
        Self { chain_spec: self.chain_spec.clone() }
    }
}

impl<ChainSpec> EthBlockAssembler<ChainSpec> {
    /// Creates a new [`EthBlockAssembler`].
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<ChainSpec: EthChainSpec + EthereumHardforks> EthBlockAssembler<ChainSpec> {
    /// Assembles a block. Accepts optional precomputed transaction root, receipt root, and logs
    /// bloom.
    pub fn assemble_block<F>(
        &self,
        input: BlockAssemblerInput<'_, '_, F>,
        transactions_root: Option<B256>,
        receipts_root: Option<B256>,
        logs_bloom: Option<Bloom>,
    ) -> Result<Block<TransactionSigned>, BlockExecutionError>
    where
        F: for<'a> BlockExecutorFactory<
                Primitives = crate::EthPrimitives,
                EvmEnv = EthEvmEnv,
                ExecutionCtx<'a> = EthBlockExecutionCtx<'a>,
            > + 'static,
    {
        let BlockAssemblerInput {
            evm_env,
            execution_ctx: ctx,
            parent,
            transactions,
            output: BlockExecutionResult { receipts, requests, gas_used, blob_gas_used },
            state_root,
            block_access_list_hash,
            ..
        } = input;
        let block = evm_env.block;

        let timestamp = block.timestamp.to::<u64>();
        let transactions_root =
            transactions_root.unwrap_or_else(|| proofs::calculate_transaction_root(&transactions));
        let receipts_root = receipts_root.unwrap_or_else(|| {
            calculate_receipt_root(&receipts.iter().map(|r| r.with_bloom_ref()).collect::<Vec<_>>())
        });
        let logs_bloom = logs_bloom
            .unwrap_or_else(|| calculate_logs_bloom(receipts.iter().flat_map(|r| r.logs())));

        let withdrawals = self
            .chain_spec
            .is_shanghai_active_at_timestamp(timestamp)
            .then(|| Withdrawals::new(ctx.withdrawals.map(|w| w.into_owned()).unwrap_or_default()));
        let withdrawals_root =
            withdrawals.as_deref().map(|w| proofs::calculate_withdrawals_root(w));
        let requests_hash = self
            .chain_spec
            .is_prague_active_at_timestamp(timestamp)
            .then(|| requests.requests_hash());
        let block_number = block.number.to::<u64>();
        let base_fee_per_gas = self
            .chain_spec
            .is_london_active_at_block(block_number)
            .then(|| block.basefee.to::<u64>());

        let mut excess_blob_gas = None;
        let mut block_blob_gas_used = None;
        if self.chain_spec.is_cancun_active_at_timestamp(timestamp) {
            block_blob_gas_used = Some(*blob_gas_used);
            excess_blob_gas = if self.chain_spec.is_cancun_active_at_timestamp(parent.timestamp) {
                parent.maybe_next_block_excess_blob_gas(
                    self.chain_spec.blob_params_at_timestamp(timestamp),
                )
            } else {
                Some(
                    alloy_eips::eip7840::BlobParams::cancun()
                        .next_block_excess_blob_gas_osaka(0, 0, 0),
                )
            };
        }

        let header = Header {
            parent_hash: ctx.parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: block.beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
            logs_bloom,
            timestamp,
            mix_hash: B256::from(block.prevrandao.to_be_bytes::<32>()),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas,
            number: block_number,
            gas_limit: block.gas_limit.to::<u64>(),
            difficulty: block.difficulty,
            gas_used: *gas_used,
            extra_data: ctx.extra_data,
            parent_beacon_block_root: ctx.parent_beacon_block_root,
            blob_gas_used: block_blob_gas_used,
            excess_blob_gas,
            requests_hash,
            block_access_list_hash,
            slot_number: ctx.slot_number,
        };

        Ok(Block {
            header,
            body: BlockBody { transactions, ommers: Default::default(), withdrawals },
        })
    }
}

impl<ChainSpec, EvmFactory> BlockAssembler<EthBlockExecutorFactory<ChainSpec, EvmFactory>>
    for EthBlockAssembler<ChainSpec>
where
    ChainSpec: EthChainSpec<Header = Header>
        + EthExecutorSpec
        + EthereumHardforks
        + core::fmt::Debug
        + Send
        + Sync
        + 'static,
    EvmFactory: Clone + core::fmt::Debug + Send + Sync + Unpin + 'static,
{
    type Block = Block<TransactionSigned>;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, EthBlockExecutorFactory<ChainSpec, EvmFactory>, Header>,
    ) -> Result<Self::Block, BlockExecutionError> {
        self.assemble_block(input, None, None, None)
    }
}
