#![cfg_attr(test, allow(dead_code))]
#![allow(unused)]
use alloc::sync::Arc;
use alloy_consensus::{proofs, Block, BlockBody, Header, TxReceipt};
use alloy_primitives::{logs_bloom, Address, B256, B64, Bytes, U256};
use reth_evm::execute::{BlockAssembler, BlockAssemblerInput};
use reth_execution_errors::BlockExecutionError;
use reth_primitives_traits::{Receipt, SignedTransaction};
use crate::ArbitrumChainSpec;
use crate::header::{ArbHeaderInfo, derive_arb_header_info_from_state, compute_nitro_mixhash, read_arbos_version};

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

        reth_tracing::tracing::debug!(
            target: "arb-evm::assemble",
            number = input.evm_env.block_env.number.saturating_to::<u64>(),
            gas_used_from_result = gas_used,
            receipts_count = receipts.len(),
            "Block assembly: gas_used from BlockExecutionResult"
        );

        let gas_adjustment = crate::get_and_clear_gas_adjustment();
        let adjusted_gas_used = if gas_adjustment != 0 {
            let new_gas = (*gas_used as i64 + gas_adjustment) as u64;
            reth_tracing::tracing::debug!(
                target: "arb-evm::assemble",
                original_gas = gas_used,
                adjustment = gas_adjustment,
                adjusted_gas = new_gas,
                "Adjusting block gas_used for early-terminated transactions"
            );
            new_gas
        } else {
            *gas_used
        };

        let transactions_root = alloy_consensus::proofs::calculate_transaction_root(&input.transactions);
        // Wrap receipts in ReceiptWithBloom before calculating root, as per EIP-2718 encoding
        let receipts_with_bloom = receipts.iter().map(|r| r.with_bloom_ref()).collect::<Vec<_>>();
        let receipts_root = alloy_consensus::proofs::calculate_receipt_root(&receipts_with_bloom);
        let logs_bloom = alloy_primitives::logs_bloom(receipts.iter().flat_map(|r| r.logs()));

        let mut header = alloy_consensus::Header {
            parent_hash: input.execution_ctx.parent_hash,
            ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
            beneficiary: input.evm_env.block_env.beneficiary,
            state_root: input.state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: None,
            logs_bloom,
            timestamp: input.evm_env.block_env.timestamp.saturating_to(),
            mix_hash: input.parent.mix_hash,
            nonce: alloy_eips::merge::BEACON_NONCE.into(),
            base_fee_per_gas: Some(input.evm_env.block_env.basefee),
            number: input.evm_env.block_env.number.saturating_to(),
            gas_limit: input.evm_env.block_env.gas_limit,
            difficulty: input.evm_env.block_env.difficulty,
            gas_used: adjusted_gas_used,
            extra_data: alloy_primitives::Bytes::from(input.parent.extra_data.clone()),
            parent_beacon_block_root: input.execution_ctx.parent_beacon_block_root,
            blob_gas_used: None,
            excess_blob_gas: None,
            requests_hash: None,
        };
        reth_tracing::tracing::debug!(
            target: "arb-evm::assemble",
            number = header.number,
            gas_used = adjusted_gas_used,
            receipts_count = receipts.len(),
            "ArbBlockAssembler: header created with gas_used"
        );
        reth_tracing::tracing::debug!(
            target: "arb-evm::assemble",
            number = header.number,
            beneficiary = %header.beneficiary,
            "ArbBlockAssembler: initial beneficiary set from evm_env"
        );
        header.difficulty = U256::from(1u64);
        header.nonce = B64::from(input.execution_ctx.delayed_messages_read.to_be_bytes()).into();

        reth_tracing::tracing::debug!(
            target: "arb-evm::assemble",
            block_number = header.number,
            exec_ctx_l1_block_num = input.execution_ctx.l1_block_number,
            exec_ctx_delayed_msgs = input.execution_ctx.delayed_messages_read,
            "Block assembly: execution context values"
        );

        if let Some(mut info) = derive_arb_header_info_from_state(&input) {
            reth_tracing::tracing::debug!(
                target: "arb-evm::assemble",
                block_number = header.number,
                send_count = info.send_count,
                l1_block_num_from_state = info.l1_block_number,
                arbos_version = info.arbos_format_version,
                send_root = %info.send_root,
                "Derived ArbHeaderInfo from state"
            );
            if info.arbos_format_version == 0 {
                if let Some(ver) = read_arbos_version(input.state_provider) {
                    info.arbos_format_version = ver;
                } else {
                    info.arbos_format_version = 10;
                }
            }
            // If the L1 block number from state is 0, use the one from execution context
            if info.l1_block_number == 0 && input.execution_ctx.l1_block_number != 0 {
                reth_tracing::tracing::warn!(
                    target: "arb-evm::assemble",
                    block_number = header.number,
                    l1_block_from_ctx = input.execution_ctx.l1_block_number,
                    "L1 block number from state is 0, using execution context value as fallback"
                );
                info.l1_block_number = input.execution_ctx.l1_block_number;
            }
            reth_tracing::tracing::debug!(
                target: "arb-evm::assemble",
                block_number = header.number,
                final_send_count = info.send_count,
                final_l1_block_num = info.l1_block_number,
                final_arbos_version = info.arbos_format_version,
                "Applying final ArbHeaderInfo to header"
            );
            info.apply_to_header(&mut header);
        } else {
            reth_tracing::tracing::error!(
                target: "arb-evm::assemble",
                block_number = header.number,
                l1_block_from_ctx = input.execution_ctx.l1_block_number,
                "Failed to derive ArbHeaderInfo from state, using fallback"
            );
            let l1_bn = input.execution_ctx.l1_block_number;
            if l1_bn != 0 {
                let ver = read_arbos_version(input.state_provider).unwrap_or(10);
                header.mix_hash = compute_nitro_mixhash(0, l1_bn, ver);
                header.extra_data = alloy_primitives::Bytes::from(vec![0u8; 32]);
            }
        }

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
    pub delayed_messages_read: u64,
    pub l1_block_number: u64,
    /// Base fee for scheduled transactions
    pub basefee: U256,
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
            delayed_messages_read: 0,
            l1_block_number: 0,
            basefee: U256::from(parent.base_fee_per_gas().unwrap_or_default()),
        }
    }
}
impl<Attrs, H, CS> reth_payload_primitives::BuildNextEnv<Attrs, H, CS> for ArbNextBlockEnvAttributes
where
    Attrs: Into<reth_payload_builder::EthPayloadBuilderAttributes> + Clone,
    H: alloy_consensus::BlockHeader,
    CS: crate::ArbitrumChainSpec,
{
    fn build_next_env(
        attrs: &Attrs,
        parent: &reth_primitives_traits::SealedHeader<H>,
        _chain_spec: &CS,
    ) -> Result<Self, reth_payload_primitives::PayloadBuilderError> {
        let attrs: reth_payload_builder::EthPayloadBuilderAttributes = attrs.clone().into();
        let mut ts = attrs.timestamp;
        let min_ts = parent.timestamp();
        if ts < min_ts {
            ts = min_ts;
        }
        Ok(ArbNextBlockEnvAttributes {
            timestamp: ts,
            suggested_fee_recipient: attrs.suggested_fee_recipient,
            prev_randao: attrs.prev_randao,
            gas_limit: parent.gas_limit(),
            withdrawals: None,
            parent_beacon_block_root: attrs.parent_beacon_block_root.or_else(|| parent.parent_beacon_block_root()),
            extra_data: alloy_primitives::Bytes::new(),
            max_fee_per_gas: None,
            blob_gas_price: None,
            delayed_messages_read: 0,
            l1_block_number: 0,
            basefee: U256::from(parent.base_fee_per_gas().unwrap_or_default()),
        })
    }
}
