use alloy_consensus::{Eip658Value, Receipt};
use alloy_evm::eth::receipt_builder::ReceiptBuilderCtx;
use alloy_op_evm::block::receipt_builder::OpReceiptBuilder;
use alloy_primitives::logs_bloom;
use op_alloy_consensus::{OpDepositReceipt, OpTxType};
use reth_evm::Evm;
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use tracing::debug;

/// A builder that operates on op-reth primitive types, specifically [`OpTransactionSigned`] and
/// [`OpReceipt`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct OpRethReceiptBuilder;

impl OpReceiptBuilder for OpRethReceiptBuilder {
    type Transaction = OpTransactionSigned;
    type Receipt = OpReceipt;

    fn build_receipt<'a, E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, OpTransactionSigned, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, OpTransactionSigned, E>> {
        let tx_hash = *ctx.tx.tx_hash();
        let tx_type = ctx.tx.tx_type();
        let execution_logs = ctx.result.logs();
        let execution_logs_len = execution_logs.len();
        let execution_logs_bloom = logs_bloom(execution_logs.iter());
        let execution_success = ctx.result.is_success();
        let execution_gas_used = ctx.result.gas_used();

        match tx_type {
            OpTxType::Deposit => {
                debug!(
                    target: "optimism::evm::receipt_builder",
                    ?tx_hash,
                    ?tx_type,
                    execution_success,
                    execution_gas_used,
                    cumulative_gas_used = ctx.cumulative_gas_used,
                    execution_logs_len,
                    execution_logs_bloom = %execution_logs_bloom,
                    "Delegating deposit receipt build"
                );
                Err(ctx)
            }
            ty => {
                let logs = ctx.result.into_logs();
                let built_logs_len = logs.len();
                let built_logs_bloom = logs_bloom(logs.iter());
                let receipt = Receipt {
                    // Success flag was added in `EIP-658: Embedding transaction status code in
                    // receipts`.
                    status: Eip658Value::Eip658(execution_success),
                    cumulative_gas_used: ctx.cumulative_gas_used,
                    logs,
                };

                let receipt = match ty {
                    OpTxType::Legacy => OpReceipt::Legacy(receipt),
                    OpTxType::Eip1559 => OpReceipt::Eip1559(receipt),
                    OpTxType::Eip2930 => OpReceipt::Eip2930(receipt),
                    OpTxType::Eip7702 => OpReceipt::Eip7702(receipt),
                    OpTxType::Deposit => unreachable!(),
                };

                debug!(
                    target: "optimism::evm::receipt_builder",
                    ?tx_hash,
                    ?tx_type,
                    execution_success,
                    execution_gas_used,
                    cumulative_gas_used = ctx.cumulative_gas_used,
                    execution_logs_len,
                    execution_logs_bloom = %execution_logs_bloom,
                    built_logs_len,
                    built_logs_bloom = %built_logs_bloom,
                    "Built non-deposit receipt"
                );

                Ok(receipt)
            }
        }
    }

    fn build_deposit_receipt(&self, inner: OpDepositReceipt) -> Self::Receipt {
        OpReceipt::Deposit(inner)
    }
}
