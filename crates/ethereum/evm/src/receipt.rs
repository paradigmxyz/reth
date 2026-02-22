use alloy_consensus::TxType;
use alloy_evm::eth::receipt_builder::{ReceiptBuilder, ReceiptBuilderCtx};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::Evm;

/// A builder that operates on Reth primitive types, specifically [`TransactionSigned`] and
/// [`Receipt`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder for RethReceiptBuilder {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn build_receipt<E: Evm>(&self, ctx: ReceiptBuilderCtx<'_, TxType, E>) -> Self::Receipt {
        let ReceiptBuilderCtx { tx_type, result, cumulative_gas_used, gas_spent, .. } = ctx;
        // EIP-7778: when active, `cumulative_gas_used` tracks gas before refunds (for block
        // accounting), but receipts must use gas after refunds (unchanged). `gas_spent` holds the
        // after-refund cumulative gas when EIP-7778 is active.
        let receipt_gas = gas_spent.unwrap_or(cumulative_gas_used);
        Receipt {
            tx_type,
            // Success flag was added in `EIP-658: Embedding transaction status code in
            // receipts`.
            success: result.is_success(),
            cumulative_gas_used: receipt_gas,
            logs: result.into_logs(),
        }
    }
}
