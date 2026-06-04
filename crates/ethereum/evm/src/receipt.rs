use alloy_consensus::TxType;
use alloy_evm::eth::receipt_builder::{ReceiptBuilder, ReceiptBuilderCtx};
use evm2::TxResult;
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::Evm;

/// A builder that operates on Reth primitive types, specifically [`TransactionSigned`] and
/// [`Receipt`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl RethReceiptBuilder {
    /// Builds a Reth receipt from an evm2 transaction result.
    pub fn build_evm2_receipt(
        &self,
        tx_type: TxType,
        result: TxResult,
        cumulative_gas_used: u64,
    ) -> Receipt {
        Receipt {
            tx_type,
            success: result.status,
            cumulative_gas_used,
            logs: result.state_changes.logs,
        }
    }
}

impl ReceiptBuilder for RethReceiptBuilder {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn build_receipt<E: Evm>(&self, ctx: ReceiptBuilderCtx<'_, TxType, E>) -> Self::Receipt {
        let ReceiptBuilderCtx { tx_type, result, cumulative_gas_used, .. } = ctx;
        Receipt {
            tx_type,
            // Success flag was added in `EIP-658: Embedding transaction status code in
            // receipts`.
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Log, LogData, B256};

    #[test]
    fn builds_receipt_from_evm2_tx_result() {
        let log = Log {
            address: address!("0000000000000000000000000000000000000001"),
            data: LogData::new_unchecked(vec![B256::ZERO], Default::default()),
        };
        let mut result = TxResult::default();
        result.status = true;
        result.state_changes.logs.push(log.clone());

        let receipt = RethReceiptBuilder.build_evm2_receipt(TxType::Eip1559, result, 42);

        assert_eq!(receipt.tx_type, TxType::Eip1559);
        assert!(receipt.success);
        assert_eq!(receipt.cumulative_gas_used, 42);
        assert_eq!(receipt.logs, vec![log]);
    }
}
