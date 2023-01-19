//! Block related models and types.

use std::ops::Range;

use crate::{
    impl_fixed_arbitrary,
    table::{Decode, Encode},
    Error,
};
use bytes::Bytes;
use reth_codecs::{main_codec, Compact};
use reth_primitives::{BlockHash, BlockNumber, Header, TxNumber, H256};
use serde::{Deserialize, Serialize};

/// Total chain number of transactions. Value for [`CumulativeTxCount`]. // TODO:
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
    /// The total number of transactions
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

    /// Return a flag whether the block is empty
    pub fn is_empty(&self) -> bool {
        self.tx_count == 0
    }
}

/// The storage representation of a block ommers.
///
/// It is stored as the headers of the block's uncles.
/// tx_amount)`.
#[derive(Debug, Default, Eq, PartialEq, Clone)]
#[main_codec]
pub struct StoredBlockOmmers {
    /// The block headers of this block's uncles.
    pub ommers: Vec<Header>,
}

/// Hash of the block header. Value for [`CanonicalHeaders`]
pub type HeaderHash = H256;

/// BlockNumber concatenated with BlockHash. Used as a key for multiple tables. Having the first
/// element as BlockNumber, helps out with querying/sorting.
///
/// Since it's used as a key, the `BlockNumber` is not compressed when encoding it.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Ord, PartialOrd)]
pub struct BlockNumHash(pub (BlockNumber, BlockHash));

impl std::fmt::Debug for BlockNumHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("").field(&self.0 .0).field(&self.0 .1).finish()
    }
}

impl BlockNumHash {
    /// Consumes `Self` and returns [`BlockNumber`], [`BlockHash`]
    pub fn take(self) -> (BlockNumber, BlockHash) {
        (self.0 .0, self.0 .1)
    }

    /// Return the block number
    pub fn number(&self) -> BlockNumber {
        self.0 .0
    }

    /// Return the block hash
    pub fn hash(&self) -> BlockHash {
        self.0 .1
    }
}

impl From<(u64, H256)> for BlockNumHash {
    fn from(tpl: (u64, H256)) -> Self {
        BlockNumHash(tpl)
    }
}

impl Encode for BlockNumHash {
    type Encoded = [u8; 40];

    fn encode(self) -> Self::Encoded {
        let number = self.0 .0;
        let hash = self.0 .1;

        let mut rnum = [0; 40];

        rnum[..8].copy_from_slice(&number.to_be_bytes());
        rnum[8..].copy_from_slice(hash.as_bytes());
        rnum
    }
}

impl Decode for BlockNumHash {
    fn decode<B: Into<Bytes>>(value: B) -> Result<Self, Error> {
        let value: bytes::Bytes = value.into();

        let num =
            u64::from_be_bytes(value.as_ref()[..8].try_into().map_err(|_| Error::DecodeError)?);
        let hash = H256::from_slice(&value.slice(8..));

        Ok(BlockNumHash((num, hash)))
    }
}

impl_fixed_arbitrary!(BlockNumHash, 40);

#[cfg(test)]
mod test {
    use crate::table::{Compress, Decompress};

    use super::*;
    use rand::{thread_rng, Rng};

    #[test]
    fn test_block_num_hash() {
        let num = 1u64;
        let hash = H256::from_low_u64_be(2);
        let key = BlockNumHash((num, hash));

        let mut bytes = [0u8; 40];
        bytes[..8].copy_from_slice(&num.to_be_bytes());
        bytes[8..].copy_from_slice(&hash.0);

        let encoded = Encode::encode(key);
        assert_eq!(encoded, bytes);

        let decoded: BlockNumHash = Decode::decode(encoded.to_vec()).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_block_num_hash_rand() {
        let mut bytes = [0u8; 40];
        thread_rng().fill(bytes.as_mut_slice());
        let key = BlockNumHash::arbitrary(&mut Unstructured::new(&bytes)).unwrap();
        assert_eq!(bytes, Encode::encode(key));
    }

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
