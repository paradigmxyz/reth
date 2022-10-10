//! Block related models and types.

use crate::kv::{
    table::{Decode, Encode},
    KVError,
};
use reth_primitives::{BlockHash, BlockNumber, H256};

/// Total chain number of transactions. Key for [`CumulativeTxCount`].
pub type NumTransactions = u64;

/// Number of transactions in the block. Value for [`BlockBodies`].
pub type NumTxesInBlock = u16;

/// Hash of the block header. Value for [`CanonicalHeaders`]
pub type HeaderHash = H256;

/// BlockNumber concatenated with BlockHash. Used as a key for multiple tables. Having the first
/// element as BlockNumber, helps out with querying/sorting.
#[derive(Debug)]
#[allow(non_camel_case_types)]
pub struct BlockNumber_BlockHash((BlockNumber, BlockHash));

impl BlockNumber_BlockHash {
    /// Consumes `Self` and returns [`BlockNumber`], [`BlockHash`]
    pub fn take(self) -> (BlockNumber, BlockHash) {
        (self.0 .0, self.0 .1)
    }
}

impl Encode for BlockNumber_BlockHash {
    type Encoded = [u8; 40];

    fn encode(self) -> Self::Encoded {
        let number = self.0 .0;
        let hash = self.0 .1;

        let mut rnum = [0; 40];

        rnum.copy_from_slice(&number.encode());
        rnum[8..].copy_from_slice(&hash.encode());
        rnum
    }
}

impl Decode for BlockNumber_BlockHash {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        Ok(BlockNumber_BlockHash((u64::decode(value)?, H256::decode(&value[8..])?)))
    }
}
