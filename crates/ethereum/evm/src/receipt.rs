use alloy_consensus::TxType;
use evm2::TxResult;
use reth_ethereum_primitives::Receipt;
use reth_evm::{ReceiptBuilder, ReceiptBuilderCtx};

/// A builder that operates on Reth primitive types, specifically [`TransactionSigned`] and
/// [`Receipt`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder<TxType, TxResult> for RethReceiptBuilder {
    type Receipt = Receipt;

    fn build_receipt(&self, ctx: ReceiptBuilderCtx<TxType, TxResult>) -> Receipt {
        let ReceiptBuilderCtx { tx_type, result, cumulative_gas_used } = ctx;
        Receipt { tx_type, success: result.status, cumulative_gas_used, logs: result.logs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Log, LogData, B256};

    #[test]
    fn builds_receipt_from_tx_result() {
        let log = Log {
            address: address!("0000000000000000000000000000000000000001"),
            data: LogData::new_unchecked(vec![B256::ZERO], Default::default()),
        };
        let mut result = TxResult { status: true, ..Default::default() };
        result.logs.push(log.clone());

        let receipt = RethReceiptBuilder.build_receipt(ReceiptBuilderCtx {
            tx_type: TxType::Eip1559,
            result,
            cumulative_gas_used: 42,
        });

        assert_eq!(receipt.tx_type, TxType::Eip1559);
        assert!(receipt.success);
        assert_eq!(receipt.cumulative_gas_used, 42);
        assert_eq!(receipt.logs, vec![log]);
    }
}
