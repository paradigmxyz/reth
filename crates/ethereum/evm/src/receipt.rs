use alloc::vec::Vec;
use alloy_consensus::TxType;
use evm2::{evm::BlockStateAccumulator, TxResult};
use reth_ethereum_primitives::Receipt;
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult};
use reth_trie_common::HashedPostState;

/// A builder that operates on Reth primitive types, specifically [`TransactionSigned`] and
/// [`Receipt`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl RethReceiptBuilder {
    /// Builds a Reth receipt from an transaction result.
    pub fn build_receipt(
        &self,
        tx_type: TxType,
        result: TxResult,
        cumulative_gas_used: u64,
    ) -> Receipt {
        Receipt { tx_type, success: result.status, cumulative_gas_used, logs: result.logs }
    }

    /// Builds a block execution output from already-built receipts and execution state.
    pub(crate) fn build_block_output(
        &self,
        receipts: Vec<Receipt>,
        state: BlockStateAccumulator,
        hashed_state: Option<HashedPostState>,
    ) -> BlockExecutionOutput<Receipt> {
        let gas_used = receipts.last().map_or(0, |receipt| receipt.cumulative_gas_used);

        BlockExecutionOutput::new(
            BlockExecutionResult {
                receipts,
                requests: Default::default(),
                gas_used,
                blob_gas_used: 0,
            },
            state,
        )
        .with_hashed_state(hashed_state)
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

        let receipt = RethReceiptBuilder.build_receipt(TxType::Eip1559, result, 42);

        assert_eq!(receipt.tx_type, TxType::Eip1559);
        assert!(receipt.success);
        assert_eq!(receipt.cumulative_gas_used, 42);
        assert_eq!(receipt.logs, vec![log]);
    }
}
