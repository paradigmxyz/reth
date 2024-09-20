use std::ops::Range;

use alloy_primitives::TxNumber;
use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::Withdrawals;
use serde::{Deserialize, Serialize};

/// Total number of transactions.
pub type NumTransactions = u64;

/// The storage of the block body indices.
///
/// It has the pointer to the transaction Number of the first
/// transaction in the block and the total number of transactions.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StoredBlockBodyIndices {
    /// The number of the first transaction in this block
    ///
    /// Note: If the block is empty, this is the number of the first transaction
    /// in the next non-empty block.
    pub first_tx_num: TxNumber,
    /// The total number of transactions in the block
    ///
    /// NOTE: Number of transitions is equal to number of transactions with
    /// additional transition for block change if block has block reward or withdrawal.
    pub tx_count: NumTransactions,
}

impl StoredBlockBodyIndices {
    /// Return the range of transaction ids for this block.
    pub const fn tx_num_range(&self) -> Range<TxNumber> {
        self.first_tx_num..self.first_tx_num + self.tx_count
    }

    /// Return the index of last transaction in this block unless the block
    /// is empty in which case it refers to the last transaction in a previous
    /// non-empty block
    pub const fn last_tx_num(&self) -> TxNumber {
        self.first_tx_num.saturating_add(self.tx_count).saturating_sub(1)
    }

    /// First transaction index.
    ///
    /// Caution: If the block is empty, this is the number of the first transaction
    /// in the next non-empty block.
    pub const fn first_tx_num(&self) -> TxNumber {
        self.first_tx_num
    }

    /// Return the index of the next transaction after this block.
    pub const fn next_tx_num(&self) -> TxNumber {
        self.first_tx_num + self.tx_count
    }

    /// Return a flag whether the block is empty
    pub const fn is_empty(&self) -> bool {
        self.tx_count == 0
    }

    /// Return number of transaction inside block
    ///
    /// NOTE: This is not the same as the number of transitions.
    pub const fn tx_count(&self) -> NumTransactions {
        self.tx_count
    }
}

/// The storage representation of block withdrawals.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StoredBlockWithdrawals {
    /// The block withdrawals.
    pub withdrawals: Withdrawals,
}

#[cfg(test)]
mod tests {
    use crate::StoredBlockBodyIndices;

    #[test]
    fn block_indices() {
        let first_tx_num = 10;
        let tx_count = 6;
        let block_indices = StoredBlockBodyIndices { first_tx_num, tx_count };

        assert_eq!(block_indices.first_tx_num(), first_tx_num);
        assert_eq!(block_indices.last_tx_num(), first_tx_num + tx_count - 1);
        assert_eq!(block_indices.next_tx_num(), first_tx_num + tx_count);
        assert_eq!(block_indices.tx_count(), tx_count);
        assert_eq!(block_indices.tx_num_range(), first_tx_num..first_tx_num + tx_count);
    }
}
