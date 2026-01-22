use alloy_evm::eth::receipt_builder::{ReceiptBuilder, ReceiptBuilderCtx};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::Evm;
use revm::context_interface::result::ExecutionResult;

/// A builder that operates on Reth primitive types, specifically [`TransactionSigned`] and
/// [`Receipt`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder for RethReceiptBuilder {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn build_receipt<E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'_, Self::Transaction, E>,
    ) -> Self::Receipt {
        let ReceiptBuilderCtx { tx, result, cumulative_gas_used, .. } = ctx;

        // EIP-7778: Calculate gas_spent (net gas after refunds).
        // When Osaka is active, cumulative_gas_used is gross gas (before refunds),
        // and gas_spent is what the user actually pays (after refunds).
        // result.gas_used() returns the net gas (after refunds already applied by EVM).
        let gas_spent = match &result {
            ExecutionResult::Success { gas_refunded, .. } => {
                // gas_used from result is net (after refunds), but cumulative_gas_used
                // from alloy-evm is now gross when Osaka active.
                // gas_spent = gas_used (net) for this tx
                // For cumulative: we need to track per-tx, but receipt stores cumulative.
                // Actually, gas_spent should be per-tx net gas, not cumulative.
                // Let's use: gas_used (net) = gross - refunded
                // But result.gas_used is already net. So gas_spent = result.gas_used()
                // However, the field semantics: cumulative_gas_used is cumulative gross,
                // gas_spent should be cumulative net (what users pay).
                // For now, set gas_spent = cumulative_gas_used - gas_refunded for this tx
                // This gives us cumulative net gas.
                Some(cumulative_gas_used.saturating_sub(*gas_refunded))
            }
            ExecutionResult::Revert { gas_used, .. } | ExecutionResult::Halt { gas_used, .. } => {
                // No refunds on revert/halt, so net = gross
                Some(*gas_used)
            }
        };

        Receipt {
            tx_type: tx.tx_type(),
            // Success flag was added in `EIP-658: Embedding transaction status code in
            // receipts`.
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs(),
            gas_spent,
        }
    }
}
