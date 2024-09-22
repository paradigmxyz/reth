mod sealed;
pub use sealed::SealedHeader;

mod error;
pub use error::HeaderError;

#[cfg(any(test, feature = "test-utils", feature = "arbitrary"))]
pub mod test_utils;

use alloy_consensus::constants::{EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH};
use alloy_eips::{
    calc_next_block_base_fee, eip1559::BaseFeeParams, merge::ALLOWED_FUTURE_BLOCK_TIME_SECONDS,
    BlockNumHash,
};
use alloy_primitives::{keccak256, Address, BlockNumber, Bloom, Bytes, B256, B64, U256};
use alloy_rlp::{length_of_length, Decodable, Encodable};
use bytes::BufMut;
use core::mem;
use reth_codecs::{add_arbitrary_tests, Compact};
use revm_primitives::{calc_blob_gasprice, calc_excess_blob_gas};
use serde::{Deserialize, Serialize};

/// Block header
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Compact)]
#[add_arbitrary_tests(rlp, 25)]
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
    ///
    /// See [EIP-4895](https://eips.ethereum.org/EIPS/eip-4895).
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
    /// The Keccak 256-bit hash of the root node of the trie structure populated with each
    /// [EIP-7685] request in the block body.
    ///
    /// [EIP-7685]: https://eips.ethereum.org/EIPS/eip-7685
    pub requests_root: Option<B256>,
    /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
    /// fewer; formally Hx.
    pub extra_data: Bytes,
}

impl AsRef<Self> for Header {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Default for Header {
    fn default() -> Self {
        Self {
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
            requests_root: None,
        }
    }
}

impl Header {
    /// Checks if the block's difficulty is set to zero, indicating a Proof-of-Stake header.
    ///
    /// This function is linked to EIP-3675, proposing the consensus upgrade to Proof-of-Stake:
    /// [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#replacing-difficulty-with-0)
    ///
    /// Verifies whether, as per the EIP, the block's difficulty is updated to zero,
    /// signifying the transition to a Proof-of-Stake mechanism.
    ///
    /// Returns `true` if the block's difficulty matches the constant zero set by the EIP.
    pub fn is_zero_difficulty(&self) -> bool {
        self.difficulty.is_zero()
    }

    /// Performs a sanity check on the extradata field of the header.
    ///
    /// # Errors
    ///
    /// Returns an error if the extradata size is larger than 100 KB.
    pub fn ensure_extradata_valid(&self) -> Result<(), HeaderError> {
        if self.extra_data.len() > 100 * 1024 {
            return Err(HeaderError::LargeExtraData)
        }
        Ok(())
    }

    /// Performs a sanity check on the block difficulty field of the header.
    ///
    /// # Errors
    ///
    /// Returns an error if the block difficulty exceeds 80 bits.
    pub fn ensure_difficulty_valid(&self) -> Result<(), HeaderError> {
        if self.difficulty.bit_len() > 80 {
            return Err(HeaderError::LargeDifficulty)
        }
        Ok(())
    }

    /// Performs combined sanity checks on multiple header fields.
    ///
    /// This method combines checks for block difficulty and extradata sizes.
    ///
    /// # Errors
    ///
    /// Returns an error if either the block difficulty exceeds 80 bits
    /// or if the extradata size is larger than 100 KB.
    pub fn ensure_well_formed(&self) -> Result<(), HeaderError> {
        self.ensure_difficulty_valid()?;
        self.ensure_extradata_valid()?;
        Ok(())
    }

    /// Checks if the block's timestamp is in the past compared to the parent block's timestamp.
    ///
    /// Note: This check is relevant only pre-merge.
    pub const fn is_timestamp_in_past(&self, parent_timestamp: u64) -> bool {
        self.timestamp <= parent_timestamp
    }

    /// Checks if the block's timestamp is in the future based on the present timestamp.
    ///
    /// Clock can drift but this can be consensus issue.
    ///
    /// Note: This check is relevant only pre-merge.
    pub const fn exceeds_allowed_future_timestamp(&self, present_timestamp: u64) -> bool {
        self.timestamp > present_timestamp + ALLOWED_FUTURE_BLOCK_TIME_SECONDS
    }

    /// Returns the parent block's number and hash
    pub const fn parent_num_hash(&self) -> BlockNumHash {
        BlockNumHash { number: self.number.saturating_sub(1), hash: self.parent_hash }
    }

    /// Heavy function that will calculate hash of data and will *not* save the change to metadata.
    /// Use [`Header::seal`], [`SealedHeader`] and unlock if you need hash to be persistent.
    pub fn hash_slow(&self) -> B256 {
        keccak256(alloy_rlp::encode(self))
    }

    /// Checks if the header is empty - has no transactions and no ommers
    pub fn is_empty(&self) -> bool {
        self.transaction_root_is_empty() &&
            self.ommers_hash_is_empty() &&
            self.withdrawals_root.map_or(true, |root| root == EMPTY_ROOT_HASH)
    }

    /// Check if the ommers hash equals to empty hash list.
    pub fn ommers_hash_is_empty(&self) -> bool {
        self.ommers_hash == EMPTY_OMMER_ROOT_HASH
    }

    /// Check if the transaction root equals to empty root.
    pub fn transaction_root_is_empty(&self) -> bool {
        self.transactions_root == EMPTY_ROOT_HASH
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
    /// See also [`Self::next_block_excess_blob_gas`]
    pub fn next_block_blob_fee(&self) -> Option<u128> {
        self.next_block_excess_blob_gas().map(calc_blob_gasprice)
    }

    /// Calculate base fee for next block according to the EIP-1559 spec.
    ///
    /// Returns a `None` if no base fee is set, no EIP-1559 support
    pub fn next_block_base_fee(&self, base_fee_params: BaseFeeParams) -> Option<u64> {
        Some(calc_next_block_base_fee(
            self.gas_used as u128,
            self.gas_limit as u128,
            self.base_fee_per_gas? as u128,
            base_fee_params,
        ) as u64)
    }

    /// Calculate excess blob gas for the next block according to the EIP-4844 spec.
    ///
    /// Returns a `None` if no excess blob gas is set, no EIP-4844 support
    pub fn next_block_excess_blob_gas(&self) -> Option<u64> {
        Some(calc_excess_blob_gas(self.excess_blob_gas?, self.blob_gas_used?))
    }

    /// Seal the header with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    #[inline]
    pub const fn seal(self, hash: B256) -> SealedHeader {
        SealedHeader::new(self, hash)
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
        length += self.parent_hash.length(); // Hash of the previous block.
        length += self.ommers_hash.length(); // Hash of uncle blocks.
        length += self.beneficiary.length(); // Address that receives rewards.
        length += self.state_root.length(); // Root hash of the state object.
        length += self.transactions_root.length(); // Root hash of transactions in the block.
        length += self.receipts_root.length(); // Hash of transaction receipts.
        length += self.logs_bloom.length(); // Data structure containing event logs.
        length += self.difficulty.length(); // Difficulty value of the block.
        length += U256::from(self.number).length(); // Block number.
        length += U256::from(self.gas_limit).length(); // Maximum gas allowed.
        length += U256::from(self.gas_used).length(); // Actual gas used.
        length += self.timestamp.length(); // Block timestamp.
        length += self.extra_data.length(); // Additional arbitrary data.
        length += self.mix_hash.length(); // Hash used for mining.
        length += B64::new(self.nonce.to_be_bytes()).length(); // Nonce for mining.

        if let Some(base_fee) = self.base_fee_per_gas {
            // Adding base fee length if it exists.
            length += U256::from(base_fee).length();
        }

        if let Some(root) = self.withdrawals_root {
            // Adding withdrawals_root length if it exists.
            length += root.length();
        }

        if let Some(blob_gas_used) = self.blob_gas_used {
            // Adding blob_gas_used length if it exists.
            length += U256::from(blob_gas_used).length();
        }

        if let Some(excess_blob_gas) = self.excess_blob_gas {
            // Adding excess_blob_gas length if it exists.
            length += U256::from(excess_blob_gas).length();
        }

        if let Some(parent_beacon_block_root) = self.parent_beacon_block_root {
            length += parent_beacon_block_root.length();
        }

        if let Some(requests_root) = self.requests_root {
            length += requests_root.length();
        }

        length
    }
}

impl Encodable for Header {
    fn encode(&self, out: &mut dyn BufMut) {
        // Create a header indicating the encoded content is a list with the payload length computed
        // from the header's payload calculation function.
        let list_header =
            alloy_rlp::Header { list: true, payload_length: self.header_payload_length() };
        list_header.encode(out);

        // Encode each header field sequentially
        self.parent_hash.encode(out); // Encode parent hash.
        self.ommers_hash.encode(out); // Encode ommer's hash.
        self.beneficiary.encode(out); // Encode beneficiary.
        self.state_root.encode(out); // Encode state root.
        self.transactions_root.encode(out); // Encode transactions root.
        self.receipts_root.encode(out); // Encode receipts root.
        self.logs_bloom.encode(out); // Encode logs bloom.
        self.difficulty.encode(out); // Encode difficulty.
        U256::from(self.number).encode(out); // Encode block number.
        U256::from(self.gas_limit).encode(out); // Encode gas limit.
        U256::from(self.gas_used).encode(out); // Encode gas used.
        self.timestamp.encode(out); // Encode timestamp.
        self.extra_data.encode(out); // Encode extra data.
        self.mix_hash.encode(out); // Encode mix hash.
        B64::new(self.nonce.to_be_bytes()).encode(out); // Encode nonce.

        // Encode base fee.
        if let Some(ref base_fee) = self.base_fee_per_gas {
            U256::from(*base_fee).encode(out);
        }

        // Encode withdrawals root.
        if let Some(ref root) = self.withdrawals_root {
            root.encode(out);
        }

        // Encode blob gas used.
        if let Some(ref blob_gas_used) = self.blob_gas_used {
            U256::from(*blob_gas_used).encode(out);
        }

        // Encode excess blob gas.
        if let Some(ref excess_blob_gas) = self.excess_blob_gas {
            U256::from(*excess_blob_gas).encode(out);
        }

        // Encode parent beacon block root.
        if let Some(ref parent_beacon_block_root) = self.parent_beacon_block_root {
            parent_beacon_block_root.encode(out);
        }

        // Encode EIP-7685 requests root
        if let Some(ref requests_root) = self.requests_root {
            requests_root.encode(out);
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
            number: u64::decode(buf)?,
            gas_limit: u64::decode(buf)?,
            gas_used: u64::decode(buf)?,
            timestamp: Decodable::decode(buf)?,
            extra_data: Decodable::decode(buf)?,
            mix_hash: Decodable::decode(buf)?,
            nonce: u64::from_be_bytes(B64::decode(buf)?.0),
            base_fee_per_gas: None,
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_root: None,
        };
        if started_len - buf.len() < rlp_head.payload_length {
            this.base_fee_per_gas = Some(u64::decode(buf)?);
        }

        // Withdrawals root for post-shanghai headers
        if started_len - buf.len() < rlp_head.payload_length {
            this.withdrawals_root = Some(Decodable::decode(buf)?);
        }

        // Blob gas used and excess blob gas for post-cancun headers
        if started_len - buf.len() < rlp_head.payload_length {
            this.blob_gas_used = Some(u64::decode(buf)?);
        }

        if started_len - buf.len() < rlp_head.payload_length {
            this.excess_blob_gas = Some(u64::decode(buf)?);
        }

        // Decode parent beacon block root.
        if started_len - buf.len() < rlp_head.payload_length {
            this.parent_beacon_block_root = Some(B256::decode(buf)?);
        }

        // Decode requests root.
        if started_len - buf.len() < rlp_head.payload_length {
            this.requests_root = Some(B256::decode(buf)?);
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

#[cfg(any(test, feature = "test-utils", feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Header {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // Generate an arbitrary header, passing it to the generate_valid_header function to make
        // sure it is valid _with respect to hardforks only_.
        let base = Self {
            parent_hash: u.arbitrary()?,
            ommers_hash: u.arbitrary()?,
            beneficiary: u.arbitrary()?,
            state_root: u.arbitrary()?,
            transactions_root: u.arbitrary()?,
            receipts_root: u.arbitrary()?,
            logs_bloom: u.arbitrary()?,
            difficulty: u.arbitrary()?,
            number: u.arbitrary()?,
            gas_limit: u.arbitrary()?,
            gas_used: u.arbitrary()?,
            timestamp: u.arbitrary()?,
            extra_data: u.arbitrary()?,
            mix_hash: u.arbitrary()?,
            nonce: u.arbitrary()?,
            base_fee_per_gas: u.arbitrary()?,
            blob_gas_used: u.arbitrary()?,
            excess_blob_gas: u.arbitrary()?,
            parent_beacon_block_root: u.arbitrary()?,
            requests_root: u.arbitrary()?,
            withdrawals_root: u.arbitrary()?,
        };

        Ok(test_utils::generate_valid_header(
            base,
            u.arbitrary()?,
            u.arbitrary()?,
            u.arbitrary()?,
            u.arbitrary()?,
        ))
    }
}

/// Trait for extracting specific Ethereum block data from a header
pub trait BlockHeader {
    /// Retrieves the beneficiary (miner) of the block
    fn beneficiary(&self) -> Address;

    /// Retrieves the difficulty of the block
    fn difficulty(&self) -> U256;

    /// Retrieves the block number
    fn number(&self) -> BlockNumber;

    /// Retrieves the gas limit of the block
    fn gas_limit(&self) -> u64;

    /// Retrieves the timestamp of the block
    fn timestamp(&self) -> u64;

    /// Retrieves the mix hash of the block
    fn mix_hash(&self) -> B256;

    /// Retrieves the base fee per gas of the block, if available
    fn base_fee_per_gas(&self) -> Option<u64>;

    /// Retrieves the excess blob gas of the block, if available
    fn excess_blob_gas(&self) -> Option<u64>;
}

impl BlockHeader for Header {
    fn beneficiary(&self) -> Address {
        self.beneficiary
    }

    fn difficulty(&self) -> U256 {
        self.difficulty
    }

    fn number(&self) -> BlockNumber {
        self.number
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn mix_hash(&self) -> B256 {
        self.mix_hash
    }

    fn base_fee_per_gas(&self) -> Option<u64> {
        self.base_fee_per_gas
    }

    fn excess_blob_gas(&self) -> Option<u64> {
        self.excess_blob_gas
    }
}
