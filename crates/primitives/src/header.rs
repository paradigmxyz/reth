use std::ops::Deref;

use crate::{BlockNumber, Bytes, H160, H256, U256};

/// Block header
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// The Keccak 256-bit hash of the parent
    /// block’s header, in its entirety; formally Hp.
    pub parent_hash: H256,
    /// The Keccak 256-bit hash of the ommers list portion of this block; formally Ho.
    pub ommers_hash: H256,
    /// The 160-bit address to which all fees collected from the successful mining of this block
    /// be transferred; formally Hc.
    pub beneficiary: H160,
    /// The Keccak 256-bit hash of the root node of the state trie, after all transactions are
    /// executed and finalisations applied; formally Hr.
    pub state_root: H256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with each
    /// transaction in the transactions list portion of the
    /// block; formally Ht.
    pub transactions_root: H256,
    /// The Keccak 256-bit hash of the root
    /// node of the trie structure populated with the receipts of each transaction in the
    /// transactions list portion of the block; formally He.
    pub receipts_root: H256,
    /// The Bloom filter composed from indexable information (logger address and log topics)
    /// contained in each log entry from the receipt of each transaction in the transactions list;
    /// formally Hb.
    pub logs_bloom: H256, // change to Bloom
    /// A scalar value corresponding to the difficulty level of this block. This can be calculated
    /// from the previous block’s difficulty level and the timestamp; formally Hd.
    pub difficulty: U256,
    /// A scalar value equal to the number of ancestor blocks. The genesis block has a number of
    /// zero; formally Hi.
    pub number: BlockNumber,
    /// A scalar value equal to the current limit of gas expenditure per block; formally Hl.
    pub gas_limit: u64,
    /// A scalar value equal to the total gas used in transactions in this block; formally Hg.
    pub gas_used: u64,
    /// A scalar value equal to the reasonable output of Unix’s time() at this block’s inception;
    /// formally Hs.
    pub timestamp: u64,
    /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
    /// fewer; formally Hx.
    pub extra_data: Bytes,
    /// A 256-bit hash which, combined with the
    /// nonce, proves that a sufficient amount of computation has been carried out on this block;
    /// formally Hm.
    pub mix_hash: H256,
    /// A 64-bit value which, combined with the mixhash, proves that a sufficient amount of
    /// computation has been carried out on this block; formally Hn.
    pub nonce: u64,
    /// A scalar representing EIP1559 base fee which can move up or down each block according
    /// to a formula which is a function of gas used in parent block and gas target
    /// (block gas limit divided by elasticity multiplier) of parent block.
    /// The algorithm results in the base fee per gas increasing when blocks are
    /// above the gas target, and decreasing when blocks are below the gas target. The base fee per
    /// gas is burned.
    pub base_fee_per_gas: Option<u64>,
}

impl Header {
    /// Heavy function that will calculate hash of data and will *not* save the change to metadata.
    /// Use lock, HeaderLocked and unlock if you need hash to be persistent.
    pub fn hash_slow(&self) -> H256 {
        todo!()
    }

    /// Calculate hash and lock the Header so that it can't be changed.
    pub fn lock(self) -> HeaderLocked {
        let hash = self.hash_slow();
        HeaderLocked { header: self, hash }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// HeaderLocked that has precalculated hash, use unlock if you want to modify header.
pub struct HeaderLocked {
    /// Locked Header fields.
    header: Header,
    /// Locked Header hash.
    hash: H256,
}

impl AsRef<Header> for HeaderLocked {
    fn as_ref(&self) -> &Header {
        &self.header
    }
}

impl Deref for HeaderLocked {
    type Target = Header;

    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

impl HeaderLocked {
    /// Extract raw header that can be modified.
    pub fn unlock(self) -> Header {
        self.header
    }

    /// Return header/block hash.
    pub fn hash(&self) -> H256 {
        self.hash
    }
}
