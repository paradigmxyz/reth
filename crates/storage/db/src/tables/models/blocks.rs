//! Block related models and types.

use reth_codecs::{main_codec, Compact};
use reth_primitives::{Header, TxNumber, Withdrawal, H256};
use std::ops::Range;
/// Total number of transactions.
pub type NumTransactions = u64;

/// The storage representation of a block.
///
/// It has the pointer to the transaction Number of the first
/// transaction in the block and the total number of transactions
#[derive(Debug, Default, Eq, PartialEq, Clone)]
#[main_codec]
pub struct StoredBlockBody {
    /// The id of the first transaction in this block
    pub start_tx_id: TxNumber,
    /// The total number of transactions in the block
    pub tx_count: NumTransactions,
}

impl StoredBlockBody {
    /// Return the range of transaction ids for this body
    pub fn tx_id_range(&self) -> Range<u64> {
        self.start_tx_id..self.start_tx_id + self.tx_count
    }

    /// Return the index of last transaction in this block unless the block
    /// is empty in which case it refers to the last transaction in a previous
    /// non-empty block
    pub fn last_tx_index(&self) -> TxNumber {
        self.start_tx_id.saturating_add(self.tx_count).saturating_sub(1)
    }

    /// First transaction index.
    pub fn first_tx_index(&self) -> TxNumber {
        self.start_tx_id
    }

    /// Return a flag whether the block is empty
    pub fn is_empty(&self) -> bool {
        self.tx_count == 0
    }

    /// Return number of transaction inside block
    pub fn tx_count(&self) -> NumTransactions {
        self.tx_count
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
