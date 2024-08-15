//! Block related models and types.

use reth_codecs::{reth_codec, Compact};
use reth_primitives::{Header, Withdrawals, B256};
use serde::{Deserialize, Serialize};

/// The storage representation of a block's ommers.
///
/// It is stored as the headers of the block's uncles.
#[reth_codec]
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct StoredBlockOmmers {
    /// The block headers of this block's uncles.
    pub ommers: Vec<Header>,
}

/// The storage representation of block withdrawals.
#[reth_codec]
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
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
