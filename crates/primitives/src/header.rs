use crate::{
    basefee::calculate_next_block_base_fee,
    constants::{EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH},
    eip4844::{calc_blob_gasprice, calculate_excess_blob_gas},
    keccak256, Address, BaseFeeParams, BlockBodyRoots, BlockHash, BlockNumHash, BlockNumber, Bloom,
    Bytes, B256, B64, U256,
};
use alloy_rlp::{length_of_length, Decodable, Encodable, EMPTY_LIST_CODE, EMPTY_STRING_CODE};
use bytes::{Buf, BufMut, BytesMut};
use reth_codecs::{add_arbitrary_tests, derive_arbitrary, main_codec, Compact};
use serde::{Deserialize, Serialize};
use std::{
    mem,
    ops::{Deref, DerefMut},
};

/// Block header
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Header {
    /// The Keccak 256-bit hash of the parent
    /// block’s header, in its entirety; formally Hp.
    pub parent_hash: B256,
    /// The Keccak 256-bit hash of the ommers list portion of this block; formally Ho.
    pub ommers_hash: B256,
    /// The 160-bit address to which all fees collected from the successful mining of this block
    /// be transferred; formally Hc.
    pub beneficiary: Address,
    /// The Keccak 256-bit hash of the root node of the state trie, after all transactions are
    /// executed and finalisations applied; formally Hr.
    pub state_root: B256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with each
    /// transaction in the transactions list portion of the block; formally Ht.
    pub transactions_root: B256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with the receipts
    /// of each transaction in the transactions list portion of the block; formally He.
    pub receipts_root: B256,
    /// The Keccak 256-bit hash of the withdrawals list portion of this block.
    /// <https://eips.ethereum.org/EIPS/eip-4895>
    pub withdrawals_root: Option<B256>,
    /// The Bloom filter composed from indexable information (logger address and log topics)
    /// contained in each log entry from the receipt of each transaction in the transactions list;
    /// formally Hb.
    pub logs_bloom: Bloom,
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
    /// A 256-bit hash which, combined with the
    /// nonce, proves that a sufficient amount of computation has been carried out on this block;
    /// formally Hm.
    pub mix_hash: B256,
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
    /// The total amount of blob gas consumed by the transactions within the block, added in
    /// EIP-4844.
    pub blob_gas_used: Option<u64>,
    /// A running total of blob gas consumed in excess of the target, prior to the block. Blocks
    /// with above-target blob gas consumption increase this value, blocks with below-target blob
    /// gas consumption decrease it (bounded at 0). This was added in EIP-4844.
    pub excess_blob_gas: Option<u64>,
    /// The hash of the parent beacon block's root is included in execution blocks, as proposed by
    /// EIP-4788.
    ///
    /// This enables trust-minimized access to consensus state, supporting staking pools, bridges,
    /// and more.
    ///
    /// The beacon roots contract handles root storage, enhancing Ethereum's functionalities.
    pub parent_beacon_block_root: Option<B256>,
    /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
    /// fewer; formally Hx.
    pub extra_data: Bytes,
}

impl Default for Header {
    fn default() -> Self {
        Header {
            parent_hash: Default::default(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Default::default(),
            state_root: EMPTY_ROOT_HASH,
            transactions_root: EMPTY_ROOT_HASH,
            receipts_root: EMPTY_ROOT_HASH,
            logs_bloom: Default::default(),
            difficulty: Default::default(),
            number: 0,
            gas_limit: 0,
            gas_used: 0,
            timestamp: 0,
            extra_data: Default::default(),
            mix_hash: Default::default(),
            nonce: 0,
            base_fee_per_gas: None,
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
        }
    }
}

impl Header {
    /// Returns the parent block's number and hash
    pub fn parent_num_hash(&self) -> BlockNumHash {
        BlockNumHash { number: self.number.saturating_sub(1), hash: self.parent_hash }
    }

    /// Heavy function that will calculate hash of data and will *not* save the change to metadata.
    /// Use [`Header::seal`], [`SealedHeader`] and unlock if you need hash to be persistent.
    pub fn hash_slow(&self) -> B256 {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        keccak256(&out)
    }

    /// Checks if the header is empty - has no transactions and no ommers
    pub fn is_empty(&self) -> bool {
        let txs_and_ommers_empty = self.transaction_root_is_empty() && self.ommers_hash_is_empty();
        if let Some(withdrawals_root) = self.withdrawals_root {
            txs_and_ommers_empty && withdrawals_root == EMPTY_ROOT_HASH
        } else {
            txs_and_ommers_empty
        }
    }

    /// Check if the ommers hash equals to empty hash list.
    pub fn ommers_hash_is_empty(&self) -> bool {
        self.ommers_hash == EMPTY_OMMER_ROOT_HASH
    }

    /// Check if the transaction root equals to empty root.
    pub fn transaction_root_is_empty(&self) -> bool {
        self.transactions_root == EMPTY_ROOT_HASH
    }

    /// Converts all roots in the header to a [BlockBodyRoots] struct.
    pub fn body_roots(&self) -> BlockBodyRoots {
        BlockBodyRoots {
            tx_root: self.transactions_root,
            ommers_hash: self.ommers_hash,
            withdrawals_root: self.withdrawals_root,
        }
    }

    /// Returns the blob fee for _this_ block according to the EIP-4844 spec.
    ///
    /// Returns `None` if `excess_blob_gas` is None
    pub fn blob_fee(&self) -> Option<u128> {
        self.excess_blob_gas.map(calc_blob_gasprice)
    }

    /// Returns the blob fee for the next block according to the EIP-4844 spec.
    ///
    /// Returns `None` if `excess_blob_gas` is None.
    ///
    /// See also [Self::next_block_excess_blob_gas]
    pub fn next_block_blob_fee(&self) -> Option<u128> {
        self.next_block_excess_blob_gas().map(calc_blob_gasprice)
    }

    /// Calculate base fee for next block according to the EIP-1559 spec.
    ///
    /// Returns a `None` if no base fee is set, no EIP-1559 support
    pub fn next_block_base_fee(&self, base_fee_params: BaseFeeParams) -> Option<u64> {
        Some(calculate_next_block_base_fee(
            self.gas_used,
            self.gas_limit,
            self.base_fee_per_gas?,
            base_fee_params,
        ))
    }

    /// Calculate excess blob gas for the next block according to the EIP-4844 spec.
    ///
    /// Returns a `None` if no excess blob gas is set, no EIP-4844 support
    pub fn next_block_excess_blob_gas(&self) -> Option<u64> {
        Some(calculate_excess_blob_gas(self.excess_blob_gas?, self.blob_gas_used?))
    }

    /// Seal the header with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    #[inline]
    pub fn seal(self, hash: B256) -> SealedHeader {
        SealedHeader { header: self, hash }
    }

    /// Calculate hash and seal the Header so that it can't be changed.
    #[inline]
    pub fn seal_slow(self) -> SealedHeader {
        let hash = self.hash_slow();
        self.seal(hash)
    }

    /// Calculate a heuristic for the in-memory size of the [Header].
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<B256>() + // parent hash
        mem::size_of::<B256>() + // ommers hash
        mem::size_of::<Address>() + // beneficiary
        mem::size_of::<B256>() + // state root
        mem::size_of::<B256>() + // transactions root
        mem::size_of::<B256>() + // receipts root
        mem::size_of::<Option<B256>>() + // withdrawals root
        mem::size_of::<Bloom>() + // logs bloom
        mem::size_of::<U256>() + // difficulty
        mem::size_of::<BlockNumber>() + // number
        mem::size_of::<u64>() + // gas limit
        mem::size_of::<u64>() + // gas used
        mem::size_of::<u64>() + // timestamp
        mem::size_of::<B256>() + // mix hash
        mem::size_of::<u64>() + // nonce
        mem::size_of::<Option<u64>>() + // base fee per gas
        mem::size_of::<Option<u64>>() + // blob gas used
        mem::size_of::<Option<u64>>() + // excess blob gas
        mem::size_of::<Option<B256>>() + // parent beacon block root
        self.extra_data.len() // extra data
    }

    fn header_payload_length(&self) -> usize {
        let mut length = 0;
        length += self.parent_hash.length();
        length += self.ommers_hash.length();
        length += self.beneficiary.length();
        length += self.state_root.length();
        length += self.transactions_root.length();
        length += self.receipts_root.length();
        length += self.logs_bloom.length();
        length += self.difficulty.length();
        length += U256::from(self.number).length();
        length += U256::from(self.gas_limit).length();
        length += U256::from(self.gas_used).length();
        length += self.timestamp.length();
        length += self.extra_data.length();
        length += self.mix_hash.length();
        length += B64::new(self.nonce.to_be_bytes()).length();

        if let Some(base_fee) = self.base_fee_per_gas {
            length += U256::from(base_fee).length();
        } else if self.withdrawals_root.is_some() ||
            self.blob_gas_used.is_some() ||
            self.excess_blob_gas.is_some() ||
            self.parent_beacon_block_root.is_some()
        {
            length += 1; // EMPTY LIST CODE
        }

        if let Some(root) = self.withdrawals_root {
            length += root.length();
        } else if self.blob_gas_used.is_some() ||
            self.excess_blob_gas.is_some() ||
            self.parent_beacon_block_root.is_some()
        {
            length += 1; // EMPTY STRING CODE
        }

        if let Some(blob_gas_used) = self.blob_gas_used {
            length += U256::from(blob_gas_used).length();
        } else if self.excess_blob_gas.is_some() || self.parent_beacon_block_root.is_some() {
            length += 1; // EMPTY LIST CODE
        }

        if let Some(excess_blob_gas) = self.excess_blob_gas {
            length += U256::from(excess_blob_gas).length();
        } else if self.parent_beacon_block_root.is_some() {
            length += 1; // EMPTY LIST CODE
        }

        // Encode parent beacon block root length. If new fields are added, the above pattern will
        // need to be repeated and placeholder length added. Otherwise, it's impossible to
        // tell _which_ fields are missing. This is mainly relevant for contrived cases
        // where a header is created at random, for example:
        //  * A header is created with a withdrawals root, but no base fee. Shanghai blocks are
        //    post-London, so this is technically not valid. However, a tool like proptest would
        //    generate a block like this.
        if let Some(parent_beacon_block_root) = self.parent_beacon_block_root {
            length += parent_beacon_block_root.length();
        }

        length
    }
}

impl Encodable for Header {
    fn encode(&self, out: &mut dyn BufMut) {
        let list_header =
            alloy_rlp::Header { list: true, payload_length: self.header_payload_length() };
        list_header.encode(out);
        self.parent_hash.encode(out);
        self.ommers_hash.encode(out);
        self.beneficiary.encode(out);
        self.state_root.encode(out);
        self.transactions_root.encode(out);
        self.receipts_root.encode(out);
        self.logs_bloom.encode(out);
        self.difficulty.encode(out);
        U256::from(self.number).encode(out);
        U256::from(self.gas_limit).encode(out);
        U256::from(self.gas_used).encode(out);
        self.timestamp.encode(out);
        self.extra_data.encode(out);
        self.mix_hash.encode(out);
        B64::new(self.nonce.to_be_bytes()).encode(out);

        // Encode base fee. Put empty list if base fee is missing,
        // but withdrawals root is present.
        if let Some(ref base_fee) = self.base_fee_per_gas {
            U256::from(*base_fee).encode(out);
        } else if self.withdrawals_root.is_some() ||
            self.blob_gas_used.is_some() ||
            self.excess_blob_gas.is_some() ||
            self.parent_beacon_block_root.is_some()
        {
            out.put_u8(EMPTY_LIST_CODE);
        }

        // Encode withdrawals root. Put empty string if withdrawals root is missing,
        // but blob gas used is present.
        if let Some(ref root) = self.withdrawals_root {
            root.encode(out);
        } else if self.blob_gas_used.is_some() ||
            self.excess_blob_gas.is_some() ||
            self.parent_beacon_block_root.is_some()
        {
            out.put_u8(EMPTY_STRING_CODE);
        }

        // Encode blob gas used. Put empty list if blob gas used is missing,
        // but excess blob gas is present.
        if let Some(ref blob_gas_used) = self.blob_gas_used {
            U256::from(*blob_gas_used).encode(out);
        } else if self.excess_blob_gas.is_some() || self.parent_beacon_block_root.is_some() {
            out.put_u8(EMPTY_LIST_CODE);
        }

        // Encode excess blob gas. Put empty list if excess blob gas is missing,
        // but parent beacon block root is present.
        if let Some(ref excess_blob_gas) = self.excess_blob_gas {
            U256::from(*excess_blob_gas).encode(out);
        } else if self.parent_beacon_block_root.is_some() {
            out.put_u8(EMPTY_LIST_CODE);
        }

        // Encode parent beacon block root. If new fields are added, the above pattern will need to
        // be repeated and placeholders added. Otherwise, it's impossible to tell _which_
        // fields are missing. This is mainly relevant for contrived cases where a header is
        // created at random, for example:
        //  * A header is created with a withdrawals root, but no base fee. Shanghai blocks are
        //    post-London, so this is technically not valid. However, a tool like proptest would
        //    generate a block like this.
        if let Some(ref parent_beacon_block_root) = self.parent_beacon_block_root {
            parent_beacon_block_root.encode(out);
        }
    }

    fn length(&self) -> usize {
        let mut length = 0;
        length += self.header_payload_length();
        length += length_of_length(length);
        length
    }
}

impl Decodable for Header {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let rlp_head = alloy_rlp::Header::decode(buf)?;
        if !rlp_head.list {
            return Err(alloy_rlp::Error::UnexpectedString)
        }
        let started_len = buf.len();
        let mut this = Self {
            parent_hash: Decodable::decode(buf)?,
            ommers_hash: Decodable::decode(buf)?,
            beneficiary: Decodable::decode(buf)?,
            state_root: Decodable::decode(buf)?,
            transactions_root: Decodable::decode(buf)?,
            receipts_root: Decodable::decode(buf)?,
            logs_bloom: Decodable::decode(buf)?,
            difficulty: Decodable::decode(buf)?,
            number: U256::decode(buf)?.to::<u64>(),
            gas_limit: U256::decode(buf)?.to::<u64>(),
            gas_used: U256::decode(buf)?.to::<u64>(),
            timestamp: Decodable::decode(buf)?,
            extra_data: Decodable::decode(buf)?,
            mix_hash: Decodable::decode(buf)?,
            nonce: u64::from_be_bytes(B64::decode(buf)?.0),
            base_fee_per_gas: None,
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
        };

        if started_len - buf.len() < rlp_head.payload_length {
            if buf.first().map(|b| *b == EMPTY_LIST_CODE).unwrap_or_default() {
                buf.advance(1)
            } else {
                this.base_fee_per_gas = Some(U256::decode(buf)?.to::<u64>());
            }
        }

        // Withdrawals root for post-shanghai headers
        if started_len - buf.len() < rlp_head.payload_length {
            if buf.first().map(|b| *b == EMPTY_STRING_CODE).unwrap_or_default() {
                buf.advance(1)
            } else {
                this.withdrawals_root = Some(Decodable::decode(buf)?);
            }
        }

        // Blob gas used and excess blob gas for post-cancun headers
        if started_len - buf.len() < rlp_head.payload_length {
            if buf.first().map(|b| *b == EMPTY_LIST_CODE).unwrap_or_default() {
                buf.advance(1)
            } else {
                this.blob_gas_used = Some(U256::decode(buf)?.to::<u64>());
            }
        }

        if started_len - buf.len() < rlp_head.payload_length {
            if buf.first().map(|b| *b == EMPTY_LIST_CODE).unwrap_or_default() {
                buf.advance(1)
            } else {
                this.excess_blob_gas = Some(U256::decode(buf)?.to::<u64>());
            }
        }

        // Decode parent beacon block root. If new fields are added, the above pattern will need to
        // be repeated and placeholders decoded. Otherwise, it's impossible to tell _which_
        // fields are missing. This is mainly relevant for contrived cases where a header is
        // created at random, for example:
        //  * A header is created with a withdrawals root, but no base fee. Shanghai blocks are
        //    post-London, so this is technically not valid. However, a tool like proptest would
        //    generate a block like this.
        if started_len - buf.len() < rlp_head.payload_length {
            this.parent_beacon_block_root = Some(B256::decode(buf)?);
        }

        let consumed = started_len - buf.len();
        if consumed != rlp_head.payload_length {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }
        Ok(this)
    }
}

/// A [`Header`] that is sealed at a precalculated hash, use [`SealedHeader::unseal()`] if you want
/// to modify header.
#[add_arbitrary_tests(rlp)]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SealedHeader {
    /// Locked Header fields.
    pub header: Header,
    /// Locked Header hash.
    pub hash: BlockHash,
}

impl SealedHeader {
    /// Extract raw header that can be modified.
    pub fn unseal(self) -> Header {
        self.header
    }

    /// This is the inverse of [Header::seal_slow] which returns the raw header and hash.
    pub fn split(self) -> (Header, BlockHash) {
        (self.header, self.hash)
    }

    /// Return header/block hash.
    pub fn hash(&self) -> BlockHash {
        self.hash
    }

    /// Return the number hash tuple.
    pub fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.number, self.hash)
    }

    /// Calculates a heuristic for the in-memory size of the [SealedHeader].
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size() + mem::size_of::<BlockHash>()
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for SealedHeader {
    type Parameters = ();
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::{any, Strategy};

        any::<(Header, BlockHash)>().prop_map(move |(header, _)| header.seal_slow()).boxed()
    }

    type Strategy = proptest::strategy::BoxedStrategy<SealedHeader>;
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for SealedHeader {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Header::arbitrary(u)?.seal_slow())
    }
}

impl Default for SealedHeader {
    fn default() -> Self {
        Header::default().seal_slow()
    }
}

impl Encodable for SealedHeader {
    fn encode(&self, out: &mut dyn BufMut) {
        self.header.encode(out);
    }
}

impl Decodable for SealedHeader {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let started_len = buf.len();

        // decode the header from temp buffer
        let header = Header::decode(b)?;

        // hash the consumed bytes, the rlp encoded header
        let consumed = started_len - b.len();
        let hash = keccak256(&buf[..consumed]);

        // update original buffer
        *buf = *b;

        Ok(Self { header, hash })
    }
}

impl AsRef<Header> for SealedHeader {
    fn as_ref(&self) -> &Header {
        &self.header
    }
}

impl Deref for SealedHeader {
    type Target = Header;

    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

impl DerefMut for SealedHeader {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.header
    }
}

/// Represents the direction for a headers request depending on the `reverse` field of the request.
/// > The response must contain a number of block headers, of rising number when reverse is 0,
/// > falling when 1
///
/// Ref: <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03>
///
/// [`HeadersDirection::Rising`] block numbers for `reverse == 0 == false`
/// [`HeadersDirection::Falling`] block numbers for `reverse == 1 == true`
///
/// See also <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03>
#[derive_arbitrary(rlp)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default, Serialize, Deserialize)]
pub enum HeadersDirection {
    /// Falling block number.
    Falling,
    /// Rising block number.
    #[default]
    Rising,
}

impl HeadersDirection {
    /// Returns true for rising block numbers
    pub fn is_rising(&self) -> bool {
        matches!(self, HeadersDirection::Rising)
    }

    /// Returns true for falling block numbers
    pub fn is_falling(&self) -> bool {
        matches!(self, HeadersDirection::Falling)
    }

    /// Converts the bool into a direction.
    ///
    /// Returns:
    ///
    /// [`HeadersDirection::Rising`] block numbers for `reverse == 0 == false`
    /// [`HeadersDirection::Falling`] block numbers for `reverse == 1 == true`
    pub fn new(reverse: bool) -> Self {
        if reverse {
            HeadersDirection::Falling
        } else {
            HeadersDirection::Rising
        }
    }
}

impl Encodable for HeadersDirection {
    fn encode(&self, out: &mut dyn BufMut) {
        bool::from(*self).encode(out)
    }

    fn length(&self) -> usize {
        bool::from(*self).length()
    }
}

impl Decodable for HeadersDirection {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let value: bool = Decodable::decode(buf)?;
        Ok(value.into())
    }
}

impl From<bool> for HeadersDirection {
    fn from(reverse: bool) -> Self {
        Self::new(reverse)
    }
}

impl From<HeadersDirection> for bool {
    fn from(value: HeadersDirection) -> Self {
        match value {
            HeadersDirection::Rising => false,
            HeadersDirection::Falling => true,
        }
    }
}

#[cfg(feature = "test-utils")]
mod ethers_compat {
    use super::*;
    use ethers_core::types::{Block, H256};

    impl From<&Block<H256>> for Header {
        fn from(block: &Block<H256>) -> Self {
            Header {
                parent_hash: block.parent_hash.0.into(),
                number: block.number.unwrap().as_u64(),
                gas_limit: block.gas_limit.as_u64(),
                difficulty: U256::from_limbs(block.difficulty.0),
                nonce: block.nonce.unwrap().to_low_u64_be(),
                extra_data: block.extra_data.0.clone().into(),
                state_root: block.state_root.0.into(),
                transactions_root: block.transactions_root.0.into(),
                receipts_root: block.receipts_root.0.into(),
                timestamp: block.timestamp.as_u64(),
                mix_hash: block.mix_hash.unwrap().0.into(),
                beneficiary: block.author.unwrap().0.into(),
                base_fee_per_gas: block.base_fee_per_gas.map(|fee| fee.as_u64()),
                ommers_hash: block.uncles_hash.0.into(),
                gas_used: block.gas_used.as_u64(),
                withdrawals_root: None,
                logs_bloom: block.logs_bloom.unwrap_or_default().0.into(),
                blob_gas_used: None,
                excess_blob_gas: None,
                parent_beacon_block_root: None,
            }
        }
    }

    impl From<&Block<H256>> for SealedHeader {
        fn from(block: &Block<H256>) -> Self {
            let header = Header::from(block);
            match block.hash {
                Some(hash) => header.seal(hash.0.into()),
                None => header.seal_slow(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bytes, Decodable, Encodable, Header, B256};
    use crate::{address, b256, bloom, bytes, hex, Address, HeadersDirection, U256};
    use std::str::FromStr;

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn test_encode_block_header() {
        let expected = hex!("f901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let header = Header {
            difficulty: U256::from(0x8ae_u64),
            number: 0xd05_u64,
            gas_limit: 0x115c_u64,
            gas_used: 0x15b3_u64,
            timestamp: 0x1a0a_u64,
            extra_data: Bytes::from_str("7788").unwrap(),
            ommers_hash: B256::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            ..Default::default()
        };
        let mut data = vec![];
        header.encode(&mut data);
        assert_eq!(hex::encode(&data), hex::encode(expected));
        assert_eq!(header.length(), data.len());
    }

    // Test vector from: https://github.com/ethereum/tests/blob/f47bbef4da376a49c8fc3166f09ab8a6d182f765/BlockchainTests/ValidBlocks/bcEIP1559/baseFee.json#L15-L36
    #[test]
    fn test_eip1559_block_header_hash() {
        let expected_hash =
            B256::from_str("6a251c7c3c5dca7b42407a3752ff48f3bbca1fab7f9868371d9918daf1988d1f")
                .unwrap();
        let header = Header {
            parent_hash: b256!("e0a94a7a3c9617401586b1a27025d2d9671332d22d540e0af72b069170380f2a"),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: address!("ba5e000000000000000000000000000000000000"),
            state_root: b256!("ec3c94b18b8a1cff7d60f8d258ec723312932928626b4c9355eb4ab3568ec7f7"),
            transactions_root: b256!("50f738580ed699f0469702c7ccc63ed2e51bc034be9479b7bff4e68dee84accf"),
            receipts_root: b256!("29b0562f7140574dd0d50dee8a271b22e1a0a7b78fca58f7c60370d8317ba2a9"),
            logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            difficulty: U256::from(0x020000),
            number: 0x01_u64,
            gas_limit: 0x016345785d8a0000_u64,
            gas_used: 0x015534_u64,
            timestamp: 0x079e,
            extra_data: bytes!("42"),
            mix_hash: b256!("0000000000000000000000000000000000000000000000000000000000000000"),
            nonce: 0,
            base_fee_per_gas: Some(0x036b_u64),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
        };
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn test_decode_block_header() {
        let data = hex!("f901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let expected = Header {
            difficulty: U256::from(0x8aeu64),
            number: 0xd05u64,
            gas_limit: 0x115cu64,
            gas_used: 0x15b3u64,
            timestamp: 0x1a0au64,
            extra_data: Bytes::from_str("7788").unwrap(),
            ommers_hash: B256::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            ..Default::default()
        };
        let header = <Header as Decodable>::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        // make sure the hash matches
        let expected_hash =
            B256::from_str("8c2f2af15b7b563b6ab1e09bed0e9caade7ed730aec98b70a993597a797579a9")
                .unwrap();
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://github.com/ethereum/tests/blob/970503935aeb76f59adfa3b3224aabf25e77b83d/BlockchainTests/ValidBlocks/bcExample/shanghaiExample.json#L15-L34
    #[test]
    fn test_decode_block_header_with_withdrawals() {
        let data = hex!("f9021ca018db39e19931515b30b16b3a92c292398039e31d6c267111529c3f2ba0a26c17a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa095efce3d6972874ca8b531b233b7a1d1ff0a56f08b20c8f1b89bef1b001194a5a071e515dd89e8a7973402c2e11646081b4e2209b2d3a1550df5095289dabcb3fba0ed9c51ea52c968e552e370a77a41dac98606e98b915092fb5f949d6452fce1c4b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff830125b882079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a027f166f1d7c789251299535cb176ba34116e44894476a7886fe5d73d9be5c973");
        let expected = Header {
            parent_hash: B256::from_str(
                "18db39e19931515b30b16b3a92c292398039e31d6c267111529c3f2ba0a26c17",
            )
            .unwrap(),
            beneficiary: Address::from_str("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba").unwrap(),
            state_root: B256::from_str(
                "95efce3d6972874ca8b531b233b7a1d1ff0a56f08b20c8f1b89bef1b001194a5",
            )
            .unwrap(),
            transactions_root: B256::from_str(
                "71e515dd89e8a7973402c2e11646081b4e2209b2d3a1550df5095289dabcb3fb",
            )
            .unwrap(),
            receipts_root: B256::from_str(
                "ed9c51ea52c968e552e370a77a41dac98606e98b915092fb5f949d6452fce1c4",
            )
            .unwrap(),
            number: 0x01,
            gas_limit: 0x7fffffffffffffff,
            gas_used: 0x0125b8,
            timestamp: 0x079e,
            extra_data: Bytes::from_str("42").unwrap(),
            mix_hash: B256::from_str(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            )
            .unwrap(),
            base_fee_per_gas: Some(0x09),
            withdrawals_root: Some(
                B256::from_str("27f166f1d7c789251299535cb176ba34116e44894476a7886fe5d73d9be5c973")
                    .unwrap(),
            ),
            ..Default::default()
        };
        let header = <Header as Decodable>::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        let expected_hash =
            B256::from_str("85fdec94c534fa0a1534720f167b899d1fc268925c71c0cbf5aaa213483f5a69")
                .unwrap();
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://github.com/ethereum/tests/blob/7e9e0940c0fcdbead8af3078ede70f969109bd85/BlockchainTests/ValidBlocks/bcExample/cancunExample.json
    #[test]
    fn test_decode_block_header_with_blob_fields_ef_tests() {
        let data = hex!("f90221a03a9b485972e7353edd9152712492f0c58d89ef80623686b6bf947a4a6dce6cb6a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa03c837fc158e3e93eafcaf2e658a02f5d8f99abc9f1c4c66cdea96c0ca26406aea04409cc4b699384ba5f8248d92b784713610c5ff9c1de51e9239da0dac76de9cea046cab26abf1047b5b119ecc2dda1296b071766c8b1307e1381fcecc90d513d86b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8302a86582079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b4218302000080");
        let expected = Header {
            parent_hash: B256::from_str(
                "3a9b485972e7353edd9152712492f0c58d89ef80623686b6bf947a4a6dce6cb6",
            )
            .unwrap(),
            ommers_hash: B256::from_str(
                "1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
            )
            .unwrap(),
            beneficiary: Address::from_str("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba").unwrap(),
            state_root: B256::from_str(
                "3c837fc158e3e93eafcaf2e658a02f5d8f99abc9f1c4c66cdea96c0ca26406ae",
            )
            .unwrap(),
            transactions_root: B256::from_str(
                "4409cc4b699384ba5f8248d92b784713610c5ff9c1de51e9239da0dac76de9ce",
            )
            .unwrap(),
            receipts_root: B256::from_str(
                "46cab26abf1047b5b119ecc2dda1296b071766c8b1307e1381fcecc90d513d86",
            )
            .unwrap(),
            logs_bloom: Default::default(),
            difficulty: U256::from(0),
            number: 0x1,
            gas_limit: 0x7fffffffffffffff,
            gas_used: 0x02a865,
            timestamp: 0x079e,
            extra_data: Bytes::from(vec![0x42]),
            mix_hash: B256::from_str(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            )
            .unwrap(),
            nonce: 0,
            base_fee_per_gas: Some(9),
            withdrawals_root: Some(
                B256::from_str("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421")
                    .unwrap(),
            ),
            blob_gas_used: Some(0x020000),
            excess_blob_gas: Some(0),
            parent_beacon_block_root: None,
        };

        let header = Header::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        let expected_hash =
            B256::from_str("0x10aca3ebb4cf6ddd9e945a5db19385f9c105ede7374380c50d56384c3d233785")
                .unwrap();
        assert_eq!(header.hash_slow(), expected_hash);
    }

    #[test]
    fn test_decode_block_header_with_blob_fields() {
        // Block from devnet-7
        let data = hex!("f90239a013a7ec98912f917b3e804654e37c9866092043c13eb8eab94eb64818e886cff5a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d4934794f97e180c050e5ab072211ad2c213eb5aee4df134a0ec229dbe85b0d3643ad0f471e6ec1a36bbc87deffbbd970762d22a53b35d068aa056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000080830305988401c9c380808464c40d5499d883010c01846765746888676f312e32302e35856c696e7578a070ccadc40b16e2094954b1064749cc6fbac783c1712f1b271a8aac3eda2f232588000000000000000007a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421808401600000");
        let expected = Header {
            parent_hash: B256::from_str(
                "13a7ec98912f917b3e804654e37c9866092043c13eb8eab94eb64818e886cff5",
            )
            .unwrap(),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: address!("f97e180c050e5ab072211ad2c213eb5aee4df134"),
            state_root: b256!("ec229dbe85b0d3643ad0f471e6ec1a36bbc87deffbbd970762d22a53b35d068a"),
            transactions_root: b256!(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
            ),
            receipts_root: b256!(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
            ),
            logs_bloom: Default::default(),
            difficulty: U256::from(0),
            number: 0x30598,
            gas_limit: 0x1c9c380,
            gas_used: 0,
            timestamp: 0x64c40d54,
            extra_data: bytes!("d883010c01846765746888676f312e32302e35856c696e7578"),
            mix_hash: b256!("70ccadc40b16e2094954b1064749cc6fbac783c1712f1b271a8aac3eda2f2325"),
            nonce: 0,
            base_fee_per_gas: Some(7),
            withdrawals_root: Some(b256!(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
            )),
            parent_beacon_block_root: None,
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0x1600000),
        };

        let header = Header::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        let expected_hash =
            b256!("539c9ea0a3ca49808799d3964b8b6607037227de26bc51073c6926963127087b");
        assert_eq!(header.hash_slow(), expected_hash);
    }

    #[test]
    fn sanity_direction() {
        let reverse = true;
        assert_eq!(HeadersDirection::Falling, reverse.into());
        assert_eq!(reverse, bool::from(HeadersDirection::Falling));

        let reverse = false;
        assert_eq!(HeadersDirection::Rising, reverse.into());
        assert_eq!(reverse, bool::from(HeadersDirection::Rising));

        let mut buf = Vec::new();
        let direction = HeadersDirection::Falling;
        direction.encode(&mut buf);
        assert_eq!(direction, HeadersDirection::decode(&mut buf.as_slice()).unwrap());

        let mut buf = Vec::new();
        let direction = HeadersDirection::Rising;
        direction.encode(&mut buf);
        assert_eq!(direction, HeadersDirection::decode(&mut buf.as_slice()).unwrap());
    }
}
