//! Block related models and types.

use reth_codecs::{main_codec, Compact};
use reth_primitives::{Header, TransitionId, TxNumber, Withdrawal, H256};
use std::ops::Range;

/// Total number of transactions.
pub type NumTransactions = u64;

/// The storage representation of a block.
///
/// It has the pointer to the transaction Number of the first
/// transaction in the block and the total number of transactions
#[derive(Debug, Default, Eq, PartialEq, Clone)]
#[main_codec]
pub struct StoredBlockMeta {
    /// The number of the first transaction in this block
    pub first_tx_num: TxNumber,
    /// The id of first transition in this block.
    pub first_transition_id: TransitionId,
    /// The total number of transactions in the block
    pub tx_count: NumTransactions,
    /// Flags if additional transition changeset of the withdrawal or uncle rewards.
    pub has_block_change: bool,
}

impl StoredBlockMeta {
    /// Return the range of transaction ids for this block.
    pub fn tx_num_range(&self) -> Range<TxNumber> {
        self.first_tx_num..self.first_tx_num + self.tx_count
    }

    /// Return the range of transition ids for this block.
    pub fn transition_range(&self) -> Range<TransitionId> {
        self.first_transition_id..self.first_transition_id + self.tx_count
    }

    /// Return transition id of the state after block executed.
    pub fn transition_after_block(&self) -> TransitionId {
        self.first_transition_id + self.tx_count + (self.has_block_change as u64)
    }

    /// Return transition id of the state at the block execution.
    pub fn transition_at_block(&self) -> TransitionId {
        self.first_transition_id
    }

    /// Return the index of last transaction in this block unless the block
    /// is empty in which case it refers to the last transaction in a previous
    /// non-empty block
    pub fn last_tx_num(&self) -> TxNumber {
        self.first_tx_num.saturating_add(self.tx_count).saturating_sub(1)
    }

    /// First transaction index.
    pub fn first_tx_num(&self) -> TxNumber {
        self.first_tx_num
    }

    /// Return the index of the next transaction after this block.
    pub fn next_tx_num(&self) -> TxNumber {
        self.first_tx_num + self.tx_count
    }

    /// Return a flag whether the block is empty
    pub fn is_empty(&self) -> bool {
        self.tx_count == 0
    }

    /// Return number of transaction inside block
    ///
    /// NOTE: This is not the same as the number of transitions.
    pub fn tx_count(&self) -> NumTransactions {
        self.tx_count
    }

    /// Return flag signifying whether the block has additional
    /// transition changeset (withdrawal or uncle/block rewards).
    pub fn has_block_change(&self) -> bool {
        self.has_block_change
    }
}

/// The storage representation of a block ommers.
///
/// It is stored as the headers of the block's uncles.
/// tx_amount)`.
#[main_codec]
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StoredBlockOmmers {
    /// The block headers of this block's uncles.
    pub ommers: Vec<Header>,
}

/// The storage representation of block withdrawals.
#[main_codec]
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StoredBlockWithdrawals {
    /// The block withdrawals.
    pub withdrawals: Vec<Withdrawal>,
}

/// Hash of the block header. Value for [`CanonicalHeaders`][crate::tables::CanonicalHeaders]
pub type HeaderHash = H256;

#[cfg(test)]
mod test {
    use crate::table::{Compress, Decompress};

    use super::*;

    #[test]
    fn test_ommer() {
        let mut ommer = StoredBlockOmmers::default();
        ommer.ommers.push(Header::default());
        ommer.ommers.push(Header::default());
        assert!(
            ommer.clone() == StoredBlockOmmers::decompress::<Vec<_>>(ommer.compress()).unwrap()
        );
    }
}
