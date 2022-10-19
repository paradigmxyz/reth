//! Block related models and types.

use crate::db::{
    table::{Decode, Encode},
    Error,
};
use bytes::Bytes;
use eyre::eyre;
use reth_primitives::{BlockHash, BlockNumber, H256};

/// Total chain number of transactions. Key for [`CumulativeTxCount`].
pub type NumTransactions = u64;

/// Number of transactions in the block. Value for [`BlockBodies`].
pub type NumTxesInBlock = u16;

/// Hash of the block header. Value for [`CanonicalHeaders`]
pub type HeaderHash = H256;

/// BlockNumber concatenated with BlockHash. Used as a key for multiple tables. Having the first
/// element as BlockNumber, helps out with querying/sorting.
///
/// Since it's used as a key, the `BlockNumber` is not compressed when encoding it.
#[derive(Debug)]
#[allow(non_camel_case_types)]
pub struct BlockNumHash((BlockNumber, BlockHash));

impl BlockNumHash {
    /// Consumes `Self` and returns [`BlockNumber`], [`BlockHash`]
    pub fn take(self) -> (BlockNumber, BlockHash) {
        (self.0 .0, self.0 .1)
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

        let num = u64::from_be_bytes(
            value.as_ref().try_into().map_err(|_| Error::Decode(eyre!("Into bytes error.")))?,
        );
        let hash = H256::decode(value.slice(8..))?;

        Ok(BlockNumHash((num, hash)))
    }
}
