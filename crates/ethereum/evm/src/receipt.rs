use alloc::vec::Vec;
use alloy_consensus::TxType;
use core::mem;
use evm2::{evm::StateChanges, TxResult};
use reth_ethereum_primitives::Receipt;
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult, Evm2BundleState};

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

    /// Builds a block execution output from evm2 transaction results.
    pub fn build_evm2_block_output(
        &self,
        block_number: u64,
        txs: impl IntoIterator<Item = (TxType, TxResult)>,
    ) -> BlockExecutionOutput<Receipt> {
        self.build_evm2_block_output_with_state_changes(
            block_number,
            txs,
            core::iter::empty::<StateChanges>(),
        )
    }

    /// Builds a block execution output from evm2 transaction results plus non-receipt state
    /// changes, such as withdrawals.
    pub fn build_evm2_block_output_with_state_changes(
        &self,
        block_number: u64,
        txs: impl IntoIterator<Item = (TxType, TxResult)>,
        extra_state_changes: impl IntoIterator<Item = StateChanges>,
    ) -> BlockExecutionOutput<Receipt> {
        self.build_evm2_block_output_with_surrounding_state_changes(
            block_number,
            core::iter::empty::<StateChanges>(),
            txs,
            extra_state_changes,
        )
    }

    /// Builds a block execution output from evm2 pre-block state changes, transaction results, and
    /// post-block state changes.
    pub fn build_evm2_block_output_with_surrounding_state_changes(
        &self,
        block_number: u64,
        pre_state_changes: impl IntoIterator<Item = StateChanges>,
        txs: impl IntoIterator<Item = (TxType, TxResult)>,
        post_state_changes: impl IntoIterator<Item = StateChanges>,
    ) -> BlockExecutionOutput<Receipt> {
        let mut receipts = Vec::new();
        let mut state_changes = Vec::new();
        let mut cumulative_gas_used = 0;

        state_changes.extend(pre_state_changes);
        for (tx_type, mut result) in txs {
            cumulative_gas_used += result.gas_used;
            let logs = mem::take(&mut result.state_changes.logs);
            receipts.push(Receipt { tx_type, success: result.status, cumulative_gas_used, logs });
            state_changes.push(result.state_changes);
        }
        state_changes.extend(post_state_changes);

        let mut state = Evm2BundleState::new(block_number);
        state.append_block(state_changes);

        BlockExecutionOutput {
            result: BlockExecutionResult {
                receipts,
                requests: Default::default(),
                gas_used: cumulative_gas_used,
                blob_gas_used: 0,
            },
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Log, LogData, B256, U256};
    use evm2::evm::{AccountInfo, Tracked};

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

    #[test]
    fn builds_block_output_from_evm2_tx_results() {
        let address = address!("0000000000000000000000000000000000000001");
        let log =
            Log { address, data: LogData::new_unchecked(vec![B256::ZERO], Default::default()) };
        let mut result = TxResult::default();
        result.status = true;
        result.gas_used = 21_000;
        result.state_changes.logs.push(log.clone());
        result.state_changes.accounts.insert(
            address,
            Tracked {
                original: None,
                current: Some(AccountInfo {
                    balance: U256::from(1),
                    nonce: 1,
                    code_hash: B256::ZERO,
                    code: None,
                    _non_exhaustive: (),
                }),
                _non_exhaustive: (),
            },
        );

        let output = RethReceiptBuilder.build_evm2_block_output(7, [(TxType::Legacy, result)]);

        assert_eq!(output.result.gas_used, 21_000);
        assert_eq!(output.result.receipts[0].logs, vec![log]);
        assert_eq!(output.state.first_block(), 7);
        assert_eq!(
            output.state.accounts().get(&address).unwrap().current.as_ref().unwrap().nonce,
            1
        );
    }

    #[test]
    fn builds_block_output_with_extra_state_changes() {
        let address = address!("0000000000000000000000000000000000000001");
        let mut extra = StateChanges::default();
        extra.accounts.insert(
            address,
            Tracked {
                original: None,
                current: Some(AccountInfo {
                    balance: U256::from(5),
                    nonce: 0,
                    code_hash: B256::ZERO,
                    code: None,
                    _non_exhaustive: (),
                }),
                _non_exhaustive: (),
            },
        );

        let output = RethReceiptBuilder.build_evm2_block_output_with_state_changes(
            7,
            core::iter::empty::<(TxType, TxResult)>(),
            [extra],
        );

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.state.accounts().get(&address).unwrap().current.as_ref().unwrap().balance,
            U256::from(5)
        );
        assert_eq!(output.state.block_reverts().len(), 1);
    }

    #[test]
    fn builds_block_output_with_surrounding_state_changes_in_order() {
        let pre_address = address!("0000000000000000000000000000000000000001");
        let tx_address = address!("0000000000000000000000000000000000000002");
        let post_address = address!("0000000000000000000000000000000000000003");

        let mut pre = StateChanges::default();
        pre.accounts.insert(
            pre_address,
            Tracked {
                original: None,
                current: Some(AccountInfo {
                    balance: U256::from(1),
                    nonce: 0,
                    code_hash: B256::ZERO,
                    code: None,
                    _non_exhaustive: (),
                }),
                _non_exhaustive: (),
            },
        );

        let mut tx = TxResult::default();
        tx.status = true;
        tx.state_changes.accounts.insert(
            tx_address,
            Tracked {
                original: None,
                current: Some(AccountInfo {
                    balance: U256::from(2),
                    nonce: 0,
                    code_hash: B256::ZERO,
                    code: None,
                    _non_exhaustive: (),
                }),
                _non_exhaustive: (),
            },
        );

        let mut post = StateChanges::default();
        post.accounts.insert(
            post_address,
            Tracked {
                original: None,
                current: Some(AccountInfo {
                    balance: U256::from(3),
                    nonce: 0,
                    code_hash: B256::ZERO,
                    code: None,
                    _non_exhaustive: (),
                }),
                _non_exhaustive: (),
            },
        );

        let output = RethReceiptBuilder.build_evm2_block_output_with_surrounding_state_changes(
            7,
            [pre],
            [(TxType::Legacy, tx)],
            [post],
        );

        assert_eq!(output.state.block_reverts().len(), 1);
        assert_eq!(output.state.accounts().len(), 3);
        assert_eq!(
            output.state.accounts().get(&pre_address).unwrap().current.as_ref().unwrap().balance,
            U256::from(1)
        );
        assert_eq!(
            output.state.accounts().get(&tx_address).unwrap().current.as_ref().unwrap().balance,
            U256::from(2)
        );
        assert_eq!(
            output.state.accounts().get(&post_address).unwrap().current.as_ref().unwrap().balance,
            U256::from(3)
        );
    }
}
