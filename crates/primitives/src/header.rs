use crate::{
    keccak256,
    proofs::{EMPTY_LIST_HASH, EMPTY_ROOT},
    BlockHash, BlockNumber, Bloom, Bytes, H160, H256, U256,
};
use bytes::{BufMut, BytesMut};
use ethers_core::types::{Block, H256 as EthersH256, H64};
use reth_codecs::{add_arbitrary_tests, derive_arbitrary, main_codec, Compact};
use reth_rlp::{length_of_length, Decodable, Encodable};
use serde::{Deserialize, Serialize};
use std::ops::Deref;

/// Describes the current head block.
///
/// The head block is the highest fully synced block.
///
/// Note: This is a slimmed down version of [Header], primarily for communicating the highest block
/// with the P2P network and the RPC.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Head {
    /// The number of the head block.
    pub number: BlockNumber,
    /// The hash of the head block.
    pub hash: H256,
    /// The difficulty of the head block.
    pub difficulty: U256,
    /// The total difficulty at the head block.
    pub total_difficulty: U256,
    /// The timestamp of the head block.
    pub timestamp: u64,
}

/// Block header
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
    /// fewer; formally Hx.
    pub extra_data: Bytes,
}

impl Default for Header {
    fn default() -> Self {
        Header {
            parent_hash: Default::default(),
            ommers_hash: EMPTY_LIST_HASH,
            beneficiary: Default::default(),
            state_root: EMPTY_ROOT,
            transactions_root: EMPTY_ROOT,
            receipts_root: EMPTY_ROOT,
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
        }
    }
}

impl Header {
    /// Heavy function that will calculate hash of data and will *not* save the change to metadata.
    /// Use [`Header::seal`], [`SealedHeader`] and unlock if you need hash to be persistent.
    pub fn hash_slow(&self) -> H256 {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        keccak256(&out)
    }

    /// Checks if the header is empty - has no transactions and no ommers
    pub fn is_empty(&self) -> bool {
        self.ommers_hash == EMPTY_LIST_HASH && self.transactions_root == EMPTY_ROOT
    }

    /// Calculate hash and seal the Header so that it can't be changed.
    pub fn seal(self) -> SealedHeader {
        let hash = self.hash_slow();
        SealedHeader { header: self, hash }
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
        length += H64::from_low_u64_be(self.nonce).length();
        length += self.base_fee_per_gas.map(|fee| U256::from(fee).length()).unwrap_or_default();
        length
    }
}

impl Encodable for Header {
    fn encode(&self, out: &mut dyn BufMut) {
        let list_header =
            reth_rlp::Header { list: true, payload_length: self.header_payload_length() };
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
        H64::from_low_u64_be(self.nonce).encode(out);
        if let Some(ref base_fee) = self.base_fee_per_gas {
            U256::from(*base_fee).encode(out);
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
    fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
        let rlp_head = reth_rlp::Header::decode(buf)?;
        if !rlp_head.list {
            return Err(reth_rlp::DecodeError::UnexpectedString)
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
            nonce: H64::decode(buf)?.to_low_u64_be(),
            base_fee_per_gas: None,
        };
        let consumed = started_len - buf.len();
        if consumed < rlp_head.payload_length {
            this.base_fee_per_gas = Some(U256::decode(buf)?.to::<u64>());
        }
        let consumed = started_len - buf.len();
        if consumed != rlp_head.payload_length {
            return Err(reth_rlp::DecodeError::ListLengthMismatch {
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
    header: Header,
    /// Locked Header hash.
    hash: BlockHash,
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for SealedHeader {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<SealedHeader>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::{any, Strategy};

        any::<(Header, BlockHash)>()
            .prop_map(move |(header, _)| {
                let hash = header.hash_slow();
                SealedHeader { header, hash }
            })
            .boxed()
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for SealedHeader {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let header = Header::arbitrary(u)?;
        let hash = header.hash_slow();
        Ok(SealedHeader { header, hash })
    }
}

impl From<Block<EthersH256>> for SealedHeader {
    fn from(block: Block<EthersH256>) -> Self {
        let header = Header {
            number: block.number.unwrap().as_u64(),
            gas_limit: block.gas_limit.as_u64(),
            difficulty: block.difficulty.into(),
            nonce: block.nonce.unwrap().to_low_u64_be(),
            extra_data: block.extra_data.0.into(),
            state_root: block.state_root.0.into(),
            timestamp: block.timestamp.as_u64(),
            mix_hash: block.mix_hash.unwrap().0.into(),
            beneficiary: block.author.unwrap().0.into(),
            base_fee_per_gas: block.base_fee_per_gas.map(|fee| fee.as_u64()),
            ..Default::default()
        };
        let hash = match block.hash {
            Some(hash) => hash.0.into(),
            None => header.hash_slow(),
        };
        SealedHeader::new(header, hash)
    }
}

impl Default for SealedHeader {
    fn default() -> Self {
        let header = Header::default();
        let hash = header.hash_slow();
        Self { header, hash }
    }
}

impl Encodable for SealedHeader {
    fn encode(&self, out: &mut dyn BufMut) {
        self.header.encode(out);
    }
}

impl Decodable for SealedHeader {
    fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
        let header = Header::decode(buf)?;
        // TODO make this more performant, we are not encoding again for a hash.
        // But i dont know how much of buf is the header or if takeing rlp::Header will
        // going to consume those buf bytes.
        let hash = header.hash_slow();
        Ok(SealedHeader { header, hash })
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

impl SealedHeader {
    /// Construct a new sealed header.
    ///
    /// Applicable when hash is known from the database provided it's not corrupted.
    pub fn new(header: Header, hash: H256) -> Self {
        Self { header, hash }
    }

    /// Extract raw header that can be modified.
    pub fn unseal(self) -> Header {
        self.header
    }

    /// Return header/block hash.
    pub fn hash(&self) -> BlockHash {
        self.hash
    }

    /// Return the number hash tuple.
    pub fn num_hash(&self) -> (BlockNumber, BlockHash) {
        (self.number, self.hash)
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
    fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
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

#[cfg(test)]
mod tests {
    use super::{Bytes, Decodable, Encodable, Header, H256};
    use crate::{Address, HeadersDirection, U256};
    use ethers_core::utils::hex::{self, FromHex};
    use std::str::FromStr;

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn test_encode_block_header() {
        let expected = hex::decode("f901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000").unwrap();
        let header = Header {
            difficulty: U256::from(0x8ae_u64),
            number: 0xd05_u64,
            gas_limit: 0x115c_u64,
            gas_used: 0x15b3_u64,
            timestamp: 0x1a0a_u64,
            extra_data: Bytes::from_str("7788").unwrap(),
            ommers_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
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
            H256::from_str("6a251c7c3c5dca7b42407a3752ff48f3bbca1fab7f9868371d9918daf1988d1f")
                .unwrap();
        let header = Header {
            parent_hash: H256::from_str("e0a94a7a3c9617401586b1a27025d2d9671332d22d540e0af72b069170380f2a").unwrap(),
            ommers_hash: H256::from_str("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347").unwrap(),
            beneficiary: Address::from_str("ba5e000000000000000000000000000000000000").unwrap(),
            state_root: H256::from_str("ec3c94b18b8a1cff7d60f8d258ec723312932928626b4c9355eb4ab3568ec7f7").unwrap(),
            transactions_root: H256::from_str("50f738580ed699f0469702c7ccc63ed2e51bc034be9479b7bff4e68dee84accf").unwrap(),
            receipts_root: H256::from_str("29b0562f7140574dd0d50dee8a271b22e1a0a7b78fca58f7c60370d8317ba2a9").unwrap(),
            logs_bloom: <[u8; 256]>::from_hex("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").unwrap().into(),
            difficulty: U256::from(0x020000),
            number: 0x01_u64,
            gas_limit: 0x016345785d8a0000_u64,
            gas_used: 0x015534_u64,
            timestamp: 0x079e,
            extra_data: Bytes::from_str("42").unwrap(),
            mix_hash: H256::from_str("0000000000000000000000000000000000000000000000000000000000000000").unwrap(),
            nonce: 0,
            base_fee_per_gas: Some(0x036b_u64),
        };
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn test_decode_block_header() {
        let data = hex::decode("f901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000").unwrap();
        let expected = Header {
            difficulty: U256::from(0x8aeu64),
            number: 0xd05u64,
            gas_limit: 0x115cu64,
            gas_used: 0x15b3u64,
            timestamp: 0x1a0au64,
            extra_data: Bytes::from_str("7788").unwrap(),
            ommers_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            ..Default::default()
        };
        let header = <Header as Decodable>::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        // make sure the hash matches
        let expected_hash = H256::from_slice(
            &hex::decode("8c2f2af15b7b563b6ab1e09bed0e9caade7ed730aec98b70a993597a797579a9")
                .unwrap(),
        );
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
