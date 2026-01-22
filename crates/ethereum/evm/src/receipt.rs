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
        // EIP-7778: gas_spent is the per-tx net gas after refunds (what user pays).
        // result.gas_used() from revm already has refunds applied.
        let gas_spent = Some(ctx.result.gas_used());

        Receipt {
            tx_type: ctx.tx_type,
            // Success flag was added in `EIP-658: Embedding transaction status code in
            // receipts`.
            success: ctx.result.is_success(),
            cumulative_gas_used: ctx.cumulative_gas_used,
            logs: ctx.result.into_logs(),
            gas_spent,
        }
    }
}
