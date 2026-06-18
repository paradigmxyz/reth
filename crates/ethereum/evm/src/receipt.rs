use alloc::vec::Vec;
use alloy_consensus::TxType;
use evm2::{
    evm::{BlockStateAccumulator, StateChangeSource, StateChanges},
    TxResult, TxResultWithState,
};
use reth_ethereum_primitives::Receipt;
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult};
use reth_trie_common::HashedPostState;

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
        Receipt { tx_type, success: result.status, cumulative_gas_used, logs: result.logs }
    }

    /// Builds a block execution output from evm2 transaction results.
    pub fn build_evm2_block_output(
        &self,
        block_number: u64,
        txs: impl IntoIterator<Item = (TxType, TxResultWithState)>,
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
        txs: impl IntoIterator<Item = (TxType, TxResultWithState)>,
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
        _block_number: u64,
        pre_state_changes: impl IntoIterator<Item = StateChanges>,
        txs: impl IntoIterator<Item = (TxType, TxResultWithState)>,
        post_state_changes: impl IntoIterator<Item = StateChanges>,
    ) -> BlockExecutionOutput<Receipt> {
        let mut receipts = Vec::new();
        let mut state = BlockStateAccumulator::new();
        let mut cumulative_gas_used = 0;

        for changes in pre_state_changes {
            append_to_block_state(&mut state, &changes);
        }
        for (tx_type, result) in txs {
            cumulative_gas_used += result.result.gas_used;
            let logs = result.result.logs;
            receipts.push(Receipt {
                tx_type,
                success: result.result.status,
                cumulative_gas_used,
                logs,
            });
            append_to_block_state(&mut state, &result.state_changes);
        }
        for changes in post_state_changes {
            append_to_block_state(&mut state, &changes);
        }

        BlockExecutionOutput::new(
            BlockExecutionResult {
                receipts,
                requests: Default::default(),
                gas_used: cumulative_gas_used,
                blob_gas_used: 0,
            },
            state,
        )
    }

    /// Builds a block execution output from result-only evm2 transaction outcomes and an
    /// accumulated evm2 block state source.
    pub fn build_evm2_block_output_from_state_source<S>(
        &self,
        _block_number: u64,
        txs: impl IntoIterator<Item = (TxType, TxResult)>,
        state_source: &S,
    ) -> BlockExecutionOutput<Receipt>
    where
        S: StateChangeSource,
    {
        let mut receipts = Vec::new();
        let mut cumulative_gas_used = 0;

        for (tx_type, outcome) in txs {
            cumulative_gas_used += outcome.gas_used;
            receipts.push(Receipt {
                tx_type,
                success: outcome.status,
                cumulative_gas_used,
                logs: outcome.logs,
            });
        }

        BlockExecutionOutput::new(
            BlockExecutionResult {
                receipts,
                requests: Default::default(),
                gas_used: cumulative_gas_used,
                blob_gas_used: 0,
            },
            block_state_from_source(state_source),
        )
    }

    /// Builds a block execution output from result-only evm2 transaction outcomes and an owned
    /// accumulated evm2 block state.
    pub fn build_evm2_block_output_from_block_state(
        &self,
        txs: impl IntoIterator<Item = (TxType, TxResult)>,
        state: BlockStateAccumulator,
    ) -> BlockExecutionOutput<Receipt> {
        self.build_evm2_block_output_from_block_state_with_hashed_state(txs, state, None)
    }

    /// Builds a block execution output from result-only evm2 transaction outcomes, an owned
    /// accumulated evm2 block state, and optional precomputed hashed post-state.
    pub fn build_evm2_block_output_from_block_state_with_hashed_state(
        &self,
        txs: impl IntoIterator<Item = (TxType, TxResult)>,
        state: BlockStateAccumulator,
        hashed_state: Option<HashedPostState>,
    ) -> BlockExecutionOutput<Receipt> {
        let mut receipts = Vec::new();
        let mut cumulative_gas_used = 0;

        for (tx_type, outcome) in txs {
            cumulative_gas_used += outcome.gas_used;
            receipts.push(Receipt {
                tx_type,
                success: outcome.status,
                cumulative_gas_used,
                logs: outcome.logs,
            });
        }

        BlockExecutionOutput::new(
            BlockExecutionResult {
                receipts,
                requests: Default::default(),
                gas_used: cumulative_gas_used,
                blob_gas_used: 0,
            },
            state,
        )
        .with_hashed_state(hashed_state)
    }
}

fn block_state_from_source<S>(source: &S) -> BlockStateAccumulator
where
    S: StateChangeSource,
{
    let mut state = BlockStateAccumulator::new();
    append_to_block_state(&mut state, source);
    state
}

fn append_to_block_state<S>(state: &mut BlockStateAccumulator, source: &S)
where
    S: StateChangeSource,
{
    match source.visit(state) {
        Ok(()) => {}
        Err(err) => match err {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Log, LogData, B256, U256};
    use evm2::evm::{AccountChange, AccountInfo};

    fn account_change(current: AccountInfo) -> AccountChange {
        let mut change = AccountChange::default();
        change.current = Some(current);
        change
    }

    #[test]
    fn builds_receipt_from_evm2_tx_result() {
        let log = Log {
            address: address!("0000000000000000000000000000000000000001"),
            data: LogData::new_unchecked(vec![B256::ZERO], Default::default()),
        };
        let mut result = TxResult { status: true, ..Default::default() };
        result.logs.push(log.clone());

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
        let mut result = TxResultWithState::default();
        result.result.status = true;
        result.result.gas_used = 21_000;
        result.result.logs.push(log.clone());
        result.state_changes.accounts.insert(
            address,
            account_change(AccountInfo {
                balance: U256::from(1),
                nonce: 1,
                code_hash: B256::ZERO,
                code: None,
                _non_exhaustive: (),
            }),
        );

        let output = RethReceiptBuilder.build_evm2_block_output(7, [(TxType::Legacy, result)]);

        assert_eq!(output.result.gas_used, 21_000);
        assert_eq!(output.result.receipts[0].logs, vec![log]);
        assert_eq!(output.account_state(&address).unwrap().current.as_ref().unwrap().nonce, 1);
    }

    #[test]
    fn builds_block_output_with_extra_state_changes() {
        let address = address!("0000000000000000000000000000000000000001");
        let mut extra = StateChanges::default();
        extra.accounts.insert(
            address,
            account_change(AccountInfo {
                balance: U256::from(5),
                nonce: 0,
                code_hash: B256::ZERO,
                code: None,
                _non_exhaustive: (),
            }),
        );

        let output = RethReceiptBuilder.build_evm2_block_output_with_state_changes(
            7,
            core::iter::empty::<(TxType, TxResultWithState)>(),
            [extra],
        );

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.account_state(&address).unwrap().current.as_ref().unwrap().balance,
            U256::from(5)
        );
    }

    #[test]
    fn builds_block_output_with_surrounding_state_changes_in_order() {
        let pre_address = address!("0000000000000000000000000000000000000001");
        let tx_address = address!("0000000000000000000000000000000000000002");
        let post_address = address!("0000000000000000000000000000000000000003");

        let mut pre = StateChanges::default();
        pre.accounts.insert(
            pre_address,
            account_change(AccountInfo {
                balance: U256::from(1),
                nonce: 0,
                code_hash: B256::ZERO,
                code: None,
                _non_exhaustive: (),
            }),
        );

        let mut tx = TxResultWithState::default();
        tx.result.status = true;
        tx.state_changes.accounts.insert(
            tx_address,
            account_change(AccountInfo {
                balance: U256::from(2),
                nonce: 0,
                code_hash: B256::ZERO,
                code: None,
                _non_exhaustive: (),
            }),
        );

        let mut post = StateChanges::default();
        post.accounts.insert(
            post_address,
            account_change(AccountInfo {
                balance: U256::from(3),
                nonce: 0,
                code_hash: B256::ZERO,
                code: None,
                _non_exhaustive: (),
            }),
        );

        let output = RethReceiptBuilder.build_evm2_block_output_with_surrounding_state_changes(
            7,
            [pre],
            [(TxType::Legacy, tx)],
            [post],
        );

        assert_eq!(output.state.accounts().count(), 3);
        assert_eq!(
            output.account_state(&pre_address).unwrap().current.as_ref().unwrap().balance,
            U256::from(1)
        );
        assert_eq!(
            output.account_state(&tx_address).unwrap().current.as_ref().unwrap().balance,
            U256::from(2)
        );
        assert_eq!(
            output.account_state(&post_address).unwrap().current.as_ref().unwrap().balance,
            U256::from(3)
        );
    }
}
