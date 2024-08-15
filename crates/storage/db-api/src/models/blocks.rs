//! Block related models and types.

use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{Header, Withdrawals, B256};
use serde::{Deserialize, Serialize};

/// The storage representation of a block's ommers.
///
/// It is stored as the headers of the block's uncles.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StoredBlockOmmers {
    /// The block headers of this block's uncles.
    pub ommers: Vec<Header>,
}

/// The storage representation of block withdrawals.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StoredBlockWithdrawals {
    /// The block withdrawals.
    pub withdrawals: Withdrawals,
}

/// Hash of the block header.
pub type HeaderHash = B256;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Compress, Decompress};

    #[test]
    fn test_ommer() {
        let mut ommer = StoredBlockOmmers::default();
        ommer.ommers.push(Header::default());
        ommer.ommers.push(Header::default());
        assert_eq!(
            ommer.clone(),
            StoredBlockOmmers::decompress::<Vec<_>>(ommer.compress()).unwrap()
        );
    }
}
