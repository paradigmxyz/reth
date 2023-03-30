//! Block related models and types.

use std::ops::Range;

use crate::{
    impl_fixed_arbitrary,
    table::{Decode, Encode},
    Error,
};
use reth_codecs::{main_codec, Compact};
use reth_primitives::{
    bytes::Bytes, BlockHash, BlockNumber, Header, TransitionId, TxNumber, Withdrawal, H256,
};
use serde::{Deserialize, Serialize};

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

/// BlockNumber concatenated with BlockHash. Used as a key for multiple tables. Having the first
/// element as BlockNumber, helps out with querying/sorting.
///
/// Since it's used as a key, the `BlockNumber` is not compressed when encoding it.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Ord, PartialOrd, Hash)]
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

impl From<u64> for BlockNumHash {
    fn from(tpl: u64) -> Self {
        BlockNumHash((tpl, H256::default()))
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
        let value: Bytes = value.into();

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
