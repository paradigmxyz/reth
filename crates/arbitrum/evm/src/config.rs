#![allow(unused)]
use alloc::sync::Arc;
use alloy_consensus::{proofs, Block, BlockBody, Header, TxReceipt};
use alloy_primitives::{logs_bloom, Address, B256, Bytes, U256};
use reth_evm::execute::{BlockAssembler, BlockAssemblerInput};
use reth_execution_errors::BlockExecutionError;
use reth_primitives_traits::{Receipt, SignedTransaction};
use crate::ArbitrumChainSpec;

pub struct ArbBlockAssembler<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> Clone for ArbBlockAssembler<ChainSpec> {
    fn clone(&self) -> Self {
        Self { chain_spec: self.chain_spec.clone() }
    }
}

impl<ChainSpec> ArbBlockAssembler<ChainSpec> {
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}
impl<ChainSpec: ArbitrumChainSpec> ArbBlockAssembler<ChainSpec> {
    fn assemble_block_inner<
        F: for<'a> alloy_evm::block::BlockExecutorFactory<
            ExecutionCtx<'a> = crate::ArbBlockExecutionCtx,
            Transaction: reth_primitives_traits::SignedTransaction,
            Receipt: reth_primitives_traits::Receipt,
        >,
    >(
        &self,
        input: reth_evm::execute::BlockAssemblerInput<'_, '_, F>,
    ) -> Result<alloy_consensus::Block<F::Transaction>, reth_execution_errors::BlockExecutionError>
    where
        F::Receipt: alloy_eips::Encodable2718 + TxReceipt
{
        let reth_execution_types::BlockExecutionResult { receipts, gas_used, .. } = input.output;

        let transactions_root = alloy_consensus::proofs::calculate_transaction_root(&input.transactions);
        let receipts_root = alloy_consensus::proofs::calculate_receipt_root(&receipts);
        let logs_bloom = alloy_primitives::logs_bloom(receipts.iter().flat_map(|r| r.logs()));

        let header = alloy_consensus::Header {
            parent_hash: input.execution_ctx.parent_hash,
            ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
            beneficiary: input.evm_env.block_env.beneficiary,
            state_root: input.state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: None,
            logs_bloom,
            timestamp: input.evm_env.block_env.timestamp.saturating_to(),
            mix_hash: input.evm_env.block_env.prevrandao.unwrap_or_default(),
            nonce: alloy_eips::merge::BEACON_NONCE.into(),
            base_fee_per_gas: Some(input.evm_env.block_env.basefee),
            number: input.evm_env.block_env.number.saturating_to(),
            gas_limit: input.evm_env.block_env.gas_limit,
            difficulty: input.evm_env.block_env.difficulty,
            gas_used: *gas_used,
            extra_data: alloy_primitives::Bytes(input.execution_ctx.extra_data.clone()),
            parent_beacon_block_root: input.execution_ctx.parent_beacon_block_root,
            blob_gas_used: None,
            excess_blob_gas: None,
            requests_hash: None,
        };

        Ok(alloy_consensus::Block::new(
            header,
            alloy_consensus::BlockBody {
                transactions: input.transactions,
                ommers: Default::default(),
                withdrawals: None,
            },
        ))
    }
}

impl<F, ChainSpec> reth_evm::execute::BlockAssembler<F> for ArbBlockAssembler<ChainSpec>
where
    ChainSpec: ArbitrumChainSpec,
    F: for<'a> alloy_evm::block::BlockExecutorFactory<
        ExecutionCtx<'a> = crate::ArbBlockExecutionCtx,
        Transaction: reth_primitives_traits::SignedTransaction,
        Receipt: reth_primitives_traits::Receipt,
    >,
    F::Receipt: alloy_eips::Encodable2718 + TxReceipt,
{
    type Block = alloy_consensus::Block<F::Transaction>;

    fn assemble_block(
        &self,
        input: reth_evm::execute::BlockAssemblerInput<'_, '_, F>,
    ) -> Result<Self::Block, reth_execution_errors::BlockExecutionError> {
        self.assemble_block_inner(input)
    }
}


#[derive(Clone, Debug, Default)]
pub struct ArbNextBlockEnvAttributes {
    pub timestamp: u64,
    pub suggested_fee_recipient: Address,
    pub prev_randao: B256,
    pub gas_limit: u64,
    pub withdrawals: Option<()>,
    pub parent_beacon_block_root: Option<B256>,
    pub extra_data: Bytes,
    pub max_fee_per_gas: Option<U256>,
    pub blob_gas_price: Option<u128>,
}
#[cfg(feature = "rpc")]
impl<H: alloy_consensus::BlockHeader>
    reth_rpc_eth_api::helpers::pending_block::BuildPendingEnv<H> for ArbNextBlockEnvAttributes
{
    fn build_pending_env(parent: &reth_primitives_traits::SealedHeader<H>) -> Self {
        Self {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: alloy_primitives::B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root().map(|_| alloy_primitives::B256::ZERO),
            withdrawals: parent.withdrawals_root().map(|_| ()),
            extra_data: alloy_primitives::Bytes::new(),
            max_fee_per_gas: None,
            blob_gas_price: None,
        }
    }
}
