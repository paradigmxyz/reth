use crate::{
    basefee::calculate_next_block_base_fee,
    constants,
    constants::{
        ALLOWED_FUTURE_BLOCK_TIME_SECONDS, EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH,
        MINIMUM_GAS_LIMIT,
    },
    eip4844::{calc_blob_gasprice, calculate_excess_blob_gas},
    keccak256, Address, BaseFeeParams, BlockHash, BlockNumHash, BlockNumber, Bloom, Bytes,
    ChainSpec, GotExpected, GotExpectedBoxed, Hardfork, B256, B64, U256,
};
use alloy_rlp::{length_of_length, Decodable, Encodable, EMPTY_LIST_CODE, EMPTY_STRING_CODE};
use bytes::{Buf, BufMut, BytesMut};
use reth_codecs::{add_arbitrary_tests, derive_arbitrary, main_codec, Compact};
use serde::{Deserialize, Serialize};
use std::{mem, ops::Deref};

/// Errors that can occur during header sanity checks.
#[derive(Debug, PartialEq)]
pub enum HeaderError {
    /// Represents an error when the block difficulty is too large.
    LargeDifficulty,
    /// Represents an error when the block extradata is too large.
    LargeExtraData,
}

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
    pub fn is_timestamp_in_past(&self, parent_timestamp: u64) -> bool {
        self.timestamp <= parent_timestamp
    }

    /// Checks if the block's timestamp is in the future based on the present timestamp.
    ///
    /// Clock can drift but this can be consensus issue.
    ///
    /// Note: This check is relevant only pre-merge.
    pub fn exceeds_allowed_future_timestamp(&self, present_timestamp: u64) -> bool {
        self.timestamp > present_timestamp + ALLOWED_FUTURE_BLOCK_TIME_SECONDS
    }

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

    /// Checks if `blob_gas_used` is present in the header.
    ///
    /// Returns `true` if `blob_gas_used` is `Some`, otherwise `false`.
    fn has_blob_gas_used(&self) -> bool {
        self.blob_gas_used.is_some()
    }

    /// Checks if `excess_blob_gas` is present in the header.
    ///
    /// Returns `true` if `excess_blob_gas` is `Some`, otherwise `false`.
    fn has_excess_blob_gas(&self) -> bool {
        self.excess_blob_gas.is_some()
    }

    // Checks if `withdrawals_root` is present in the header.
    ///
    /// Returns `true` if `withdrawals_root` is `Some`, otherwise `false`.
    fn has_withdrawals_root(&self) -> bool {
        self.withdrawals_root.is_some()
    }

    /// Checks if `parent_beacon_block_root` is present in the header.
    ///
    /// Returns `true` if `parent_beacon_block_root` is `Some`, otherwise `false`.
    fn has_parent_beacon_block_root(&self) -> bool {
        self.parent_beacon_block_root.is_some()
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
        } else if self.has_withdrawals_root() ||
            self.has_blob_gas_used() ||
            self.has_excess_blob_gas() ||
            self.has_parent_beacon_block_root()
        {
            // Placeholder code for empty lists.
            length += 1;
        }

        if let Some(root) = self.withdrawals_root {
            // Adding withdrawals_root length if it exists.
            length += root.length();
        } else if self.has_blob_gas_used() ||
            self.has_excess_blob_gas() ||
            self.has_parent_beacon_block_root()
        {
            // Placeholder code for a missing string value.
            length += 1;
        }

        if let Some(blob_gas_used) = self.blob_gas_used {
            // Adding blob_gas_used length if it exists.
            length += U256::from(blob_gas_used).length();
        } else if self.has_excess_blob_gas() || self.has_parent_beacon_block_root() {
            // Placeholder code for empty lists.
            length += 1;
        }

        if let Some(excess_blob_gas) = self.excess_blob_gas {
            // Adding excess_blob_gas length if it exists.
            length += U256::from(excess_blob_gas).length();
        } else if self.has_parent_beacon_block_root() {
            // Placeholder code for empty lists.
            length += 1;
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

        // The following code is needed only to handle proptest-generated headers that are
        // technically invalid.
        //
        // TODO: make proptest generate more valid headers, ie if there is no base fee, there
        // should be no withdrawals root or any future fork field.

        // Encode base fee. Put empty list if base fee is missing,
        // but withdrawals root is present.
        if let Some(ref base_fee) = self.base_fee_per_gas {
            U256::from(*base_fee).encode(out);
        } else if self.has_withdrawals_root() ||
            self.has_blob_gas_used() ||
            self.has_excess_blob_gas() ||
            self.has_parent_beacon_block_root()
        {
            out.put_u8(EMPTY_LIST_CODE);
        }

        // Encode withdrawals root. Put empty string if withdrawals root is missing,
        // but blob gas used is present.
        if let Some(ref root) = self.withdrawals_root {
            root.encode(out);
        } else if self.has_blob_gas_used() ||
            self.has_excess_blob_gas() ||
            self.has_parent_beacon_block_root()
        {
            out.put_u8(EMPTY_STRING_CODE);
        }

        // Encode blob gas used. Put empty list if blob gas used is missing,
        // but excess blob gas is present.
        if let Some(ref blob_gas_used) = self.blob_gas_used {
            U256::from(*blob_gas_used).encode(out);
        } else if self.has_excess_blob_gas() || self.has_parent_beacon_block_root() {
            out.put_u8(EMPTY_LIST_CODE);
        }

        // Encode excess blob gas. Put empty list if excess blob gas is missing,
        // but parent beacon block root is present.
        if let Some(ref excess_blob_gas) = self.excess_blob_gas {
            U256::from(*excess_blob_gas).encode(out);
        } else if self.has_parent_beacon_block_root() {
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
        };

        if started_len - buf.len() < rlp_head.payload_length {
            if buf.first().map(|b| *b == EMPTY_LIST_CODE).unwrap_or_default() {
                buf.advance(1)
            } else {
                this.base_fee_per_gas = Some(u64::decode(buf)?);
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
                this.blob_gas_used = Some(u64::decode(buf)?);
            }
        }

        if started_len - buf.len() < rlp_head.payload_length {
            if buf.first().map(|b| *b == EMPTY_LIST_CODE).unwrap_or_default() {
                buf.advance(1)
            } else {
                this.excess_blob_gas = Some(u64::decode(buf)?);
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

/// Errors that can occur during header sanity checks.
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum HeaderValidationError {
    /// Error when the block number does not match the parent block number.
    #[error(
        "block number {block_number} does not match parent block number {parent_block_number}"
    )]
    ParentBlockNumberMismatch {
        /// The parent block number.
        parent_block_number: BlockNumber,
        /// The block number.
        block_number: BlockNumber,
    },

    /// Error when the parent hash does not match the expected parent hash.
    #[error("mismatched parent hash: {0}")]
    ParentHashMismatch(GotExpectedBoxed<B256>),

    /// Error when the block timestamp is in the past compared to the parent timestamp.
    #[error("block timestamp {timestamp} is in the past compared to the parent timestamp {parent_timestamp}")]
    TimestampIsInPast {
        /// The parent block's timestamp.
        parent_timestamp: u64,
        /// The block's timestamp.
        timestamp: u64,
    },

    /// Error when the base fee is missing.
    #[error("base fee missing")]
    BaseFeeMissing,

    /// Error when the block's base fee is different from the expected base fee.
    #[error("block base fee mismatch: {0}")]
    BaseFeeDiff(GotExpected<u64>),

    /// Error when the child gas limit exceeds the maximum allowed decrease.
    #[error("child gas_limit {child_gas_limit} max decrease is {parent_gas_limit}/1024")]
    GasLimitInvalidDecrease {
        /// The parent gas limit.
        parent_gas_limit: u64,
        /// The child gas limit.
        child_gas_limit: u64,
    },

    /// Error when the child gas limit exceeds the maximum allowed increase.
    #[error("child gas_limit {child_gas_limit} max increase is {parent_gas_limit}/1024")]
    GasLimitInvalidIncrease {
        /// The parent gas limit.
        parent_gas_limit: u64,
        /// The child gas limit.
        child_gas_limit: u64,
    },

    /// Error indicating that the child gas limit is below the minimum allowed limit.
    ///
    /// This error occurs when the child gas limit is less than the specified minimum gas limit.
    #[error("child gas limit {child_gas_limit} is below the minimum allowed limit ({MINIMUM_GAS_LIMIT})")]
    GasLimitInvalidMinimum {
        /// The child gas limit.
        child_gas_limit: u64,
    },

    /// Error when blob gas used is missing.
    #[error("missing blob gas used")]
    BlobGasUsedMissing,

    /// Error when excess blob gas is missing.
    #[error("missing excess blob gas")]
    ExcessBlobGasMissing,

    /// Error when there is an invalid excess blob gas.
    #[error(
        "invalid excess blob gas: {diff}; \
         parent excess blob gas: {parent_excess_blob_gas}, \
         parent blob gas used: {parent_blob_gas_used}"
    )]
    ExcessBlobGasDiff {
        /// The excess blob gas diff.
        diff: GotExpected<u64>,
        /// The parent excess blob gas.
        parent_excess_blob_gas: u64,
        /// The parent blob gas used.
        parent_blob_gas_used: u64,
    },
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

impl SealedHeader {
    /// Creates the sealed header with the corresponding block hash.
    #[inline]
    pub const fn new(header: Header, hash: BlockHash) -> Self {
        Self { header, hash }
    }

    /// Returns the sealed Header fields.
    #[inline]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Returns header/block hash.
    #[inline]
    pub const fn hash(&self) -> BlockHash {
        self.hash
    }

    /// Updates the block header.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_header(&mut self, header: Header) {
        self.header = header
    }

    /// Updates the block hash.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_hash(&mut self, hash: BlockHash) {
        self.hash = hash
    }

    /// Updates the parent block hash.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_parent_hash(&mut self, hash: BlockHash) {
        self.header.parent_hash = hash
    }

    /// Updates the block number.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_block_number(&mut self, number: BlockNumber) {
        self.header.number = number;
    }

    /// Updates the block state root.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_state_root(&mut self, state_root: B256) {
        self.header.state_root = state_root;
    }

    /// Updates the block difficulty.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_difficulty(&mut self, difficulty: U256) {
        self.header.difficulty = difficulty;
    }

    /// Checks the gas limit for consistency between parent and self headers.
    ///
    /// The maximum allowable difference between self and parent gas limits is determined by the
    /// parent's gas limit divided by the elasticity multiplier (1024).
    ///
    /// This check is skipped if the Optimism flag is enabled in the chain spec, as gas limits on
    /// Optimism can adjust instantly.
    #[inline(always)]
    fn validate_gas_limit(
        &self,
        parent: &SealedHeader,
        chain_spec: &ChainSpec,
    ) -> Result<(), HeaderValidationError> {
        // Determine the parent gas limit, considering elasticity multiplier on the London fork.
        let mut parent_gas_limit = parent.gas_limit;
        if chain_spec.fork(Hardfork::London).transitions_at_block(self.number) {
            parent_gas_limit =
                parent.gas_limit * chain_spec.base_fee_params(self.timestamp).elasticity_multiplier;
        }

        // Check for an increase in gas limit beyond the allowed threshold.
        if self.gas_limit > parent_gas_limit {
            if self.gas_limit - parent_gas_limit >= parent_gas_limit / 1024 {
                return Err(HeaderValidationError::GasLimitInvalidIncrease {
                    parent_gas_limit,
                    child_gas_limit: self.gas_limit,
                })
            }
        }
        // Check for a decrease in gas limit beyond the allowed threshold.
        else if parent_gas_limit - self.gas_limit >= parent_gas_limit / 1024 {
            return Err(HeaderValidationError::GasLimitInvalidDecrease {
                parent_gas_limit,
                child_gas_limit: self.gas_limit,
            })
        }
        // Check if the self gas limit is below the minimum required limit.
        else if self.gas_limit < MINIMUM_GAS_LIMIT {
            return Err(HeaderValidationError::GasLimitInvalidMinimum {
                child_gas_limit: self.gas_limit,
            })
        }

        Ok(())
    }

    /// Validates the integrity and consistency of a sealed block header in relation to its parent
    /// header.
    ///
    /// This function checks various properties of the sealed header against its parent header and
    /// the chain specification. It ensures that the block forms a valid and secure continuation
    /// of the blockchain.
    ///
    /// ## Arguments
    ///
    /// * `parent` - The sealed header of the parent block.
    /// * `chain_spec` - The chain specification providing configuration parameters for the
    ///   blockchain.
    ///
    /// ## Errors
    ///
    /// Returns a [`HeaderValidationError`] if any validation check fails, indicating specific
    /// issues with the sealed header. The possible errors include mismatched block numbers,
    /// parent hash mismatches, timestamp inconsistencies, gas limit violations, base fee
    /// discrepancies (for EIP-1559), and errors related to the blob gas fields (EIP-4844).
    ///
    /// ## Note
    ///
    /// Some checks, such as gas limit validation, are conditionally skipped based on the presence
    /// of certain features (e.g., Optimism feature) or the activation of specific hardforks.
    pub fn validate_against_parent(
        &self,
        parent: &SealedHeader,
        chain_spec: &ChainSpec,
    ) -> Result<(), HeaderValidationError> {
        // Parent number is consistent.
        if parent.number + 1 != self.number {
            return Err(HeaderValidationError::ParentBlockNumberMismatch {
                parent_block_number: parent.number,
                block_number: self.number,
            })
        }

        if parent.hash != self.parent_hash {
            return Err(HeaderValidationError::ParentHashMismatch(
                GotExpected { got: self.parent_hash, expected: parent.hash }.into(),
            ))
        }

        // timestamp in past check
        if self.header.is_timestamp_in_past(parent.timestamp) {
            return Err(HeaderValidationError::TimestampIsInPast {
                parent_timestamp: parent.timestamp,
                timestamp: self.timestamp,
            })
        }

        // TODO Check difficulty increment between parent and self
        // Ace age did increment it by some formula that we need to follow.

        cfg_if::cfg_if! {
            if #[cfg(feature = "optimism")] {
                // On Optimism, the gas limit can adjust instantly, so we skip this check
                // if the optimism feature is enabled in the chain spec.
                if !chain_spec.is_optimism() {
                    self.validate_gas_limit(parent, chain_spec)?;
                }
            } else {
                self.validate_gas_limit(parent, chain_spec)?;
            }
        }

        // EIP-1559 check base fee
        if chain_spec.fork(Hardfork::London).active_at_block(self.number) {
            let base_fee = self.base_fee_per_gas.ok_or(HeaderValidationError::BaseFeeMissing)?;

            let expected_base_fee =
                if chain_spec.fork(Hardfork::London).transitions_at_block(self.number) {
                    constants::EIP1559_INITIAL_BASE_FEE
                } else {
                    // This BaseFeeMissing will not happen as previous blocks are checked to have
                    // them.
                    parent
                        .next_block_base_fee(chain_spec.base_fee_params(self.timestamp))
                        .ok_or(HeaderValidationError::BaseFeeMissing)?
                };
            if expected_base_fee != base_fee {
                return Err(HeaderValidationError::BaseFeeDiff(GotExpected {
                    expected: expected_base_fee,
                    got: base_fee,
                }))
            }
        }

        // ensure that the blob gas fields for this block
        if chain_spec.fork(Hardfork::Cancun).active_at_timestamp(self.timestamp) {
            self.validate_4844_header_against_parent(parent)?;
        }

        Ok(())
    }

    /// Validates that the EIP-4844 header fields are correct with respect to the parent block. This
    /// ensures that the `blob_gas_used` and `excess_blob_gas` fields exist in the child header, and
    /// that the `excess_blob_gas` field matches the expected `excess_blob_gas` calculated from the
    /// parent header fields.
    pub fn validate_4844_header_against_parent(
        &self,
        parent: &SealedHeader,
    ) -> Result<(), HeaderValidationError> {
        // From [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension):
        //
        // > For the first post-fork block, both parent.blob_gas_used and parent.excess_blob_gas
        // > are evaluated as 0.
        //
        // This means in the first post-fork block, calculate_excess_blob_gas will return 0.
        let parent_blob_gas_used = parent.blob_gas_used.unwrap_or(0);
        let parent_excess_blob_gas = parent.excess_blob_gas.unwrap_or(0);

        if self.blob_gas_used.is_none() {
            return Err(HeaderValidationError::BlobGasUsedMissing)
        }
        let excess_blob_gas =
            self.excess_blob_gas.ok_or(HeaderValidationError::ExcessBlobGasMissing)?;

        let expected_excess_blob_gas =
            calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used);
        if expected_excess_blob_gas != excess_blob_gas {
            return Err(HeaderValidationError::ExcessBlobGasDiff {
                diff: GotExpected { got: excess_blob_gas, expected: expected_excess_blob_gas },
                parent_excess_blob_gas,
                parent_blob_gas_used,
            })
        }

        Ok(())
    }

    /// Extract raw header that can be modified.
    pub fn unseal(self) -> Header {
        self.header
    }

    /// This is the inverse of [Header::seal_slow] which returns the raw header and hash.
    pub fn split(self) -> (Header, BlockHash) {
        (self.header, self.hash)
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
    use crate::{
        address, b256, bloom, bytes, header::MINIMUM_GAS_LIMIT, hex, Address, ChainSpec,
        HeaderValidationError, HeadersDirection, SealedHeader, U256,
    };
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

    #[test]
    fn test_decode_block_header_with_invalid_blob_gas_used() {
        // This should error because the blob_gas_used is too large
        let data = hex!("f90242a013a7ec98912f917b3e804654e37c9866092043c13eb8eab94eb64818e886cff5a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d4934794f97e180c050e5ab072211ad2c213eb5aee4df134a0ec229dbe85b0d3643ad0f471e6ec1a36bbc87deffbbd970762d22a53b35d068aa056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000080830305988401c9c380808464c40d5499d883010c01846765746888676f312e32302e35856c696e7578a070ccadc40b16e2094954b1064749cc6fbac783c1712f1b271a8aac3eda2f232588000000000000000007a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421891122334455667788998401600000");
        Header::decode(&mut data.as_slice())
            .expect_err("blob_gas_used size should make this header decoding fail");
    }

    #[test]
    fn test_valid_gas_limit_increase() {
        let parent = SealedHeader {
            header: Header { gas_limit: 1024 * 10, ..Default::default() },
            ..Default::default()
        };
        let child = SealedHeader {
            header: Header { gas_limit: parent.header.gas_limit + 5, ..Default::default() },
            ..Default::default()
        };
        let chain_spec = ChainSpec::default();

        assert_eq!(child.validate_gas_limit(&parent, &chain_spec), Ok(()));
    }

    #[test]
    fn test_gas_limit_below_minimum() {
        let parent = SealedHeader {
            header: Header { gas_limit: MINIMUM_GAS_LIMIT, ..Default::default() },
            ..Default::default()
        };
        let child = SealedHeader {
            header: Header { gas_limit: MINIMUM_GAS_LIMIT - 1, ..Default::default() },
            ..Default::default()
        };
        let chain_spec = ChainSpec::default();

        assert_eq!(
            child.validate_gas_limit(&parent, &chain_spec),
            Err(HeaderValidationError::GasLimitInvalidMinimum { child_gas_limit: child.gas_limit })
        );
    }

    #[test]
    fn test_invalid_gas_limit_increase_exceeding_limit() {
        let gas_limit = 1024 * 10;
        let parent = SealedHeader {
            header: Header { gas_limit, ..Default::default() },
            ..Default::default()
        };
        let child = SealedHeader {
            header: Header {
                gas_limit: parent.header.gas_limit + parent.header.gas_limit / 1024 + 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let chain_spec = ChainSpec::default();

        assert_eq!(
            child.validate_gas_limit(&parent, &chain_spec),
            Err(HeaderValidationError::GasLimitInvalidIncrease {
                parent_gas_limit: parent.header.gas_limit,
                child_gas_limit: child.header.gas_limit,
            })
        );
    }

    #[test]
    fn test_valid_gas_limit_decrease_within_limit() {
        let gas_limit = 1024 * 10;
        let parent = SealedHeader {
            header: Header { gas_limit, ..Default::default() },
            ..Default::default()
        };
        let child = SealedHeader {
            header: Header { gas_limit: parent.header.gas_limit - 5, ..Default::default() },
            ..Default::default()
        };
        let chain_spec = ChainSpec::default();

        assert_eq!(child.validate_gas_limit(&parent, &chain_spec), Ok(()));
    }

    #[test]
    fn test_invalid_gas_limit_decrease_exceeding_limit() {
        let gas_limit = 1024 * 10;
        let parent = SealedHeader {
            header: Header { gas_limit, ..Default::default() },
            ..Default::default()
        };
        let child = SealedHeader {
            header: Header {
                gas_limit: parent.header.gas_limit - parent.header.gas_limit / 1024 - 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let chain_spec = ChainSpec::default();

        assert_eq!(
            child.validate_gas_limit(&parent, &chain_spec),
            Err(HeaderValidationError::GasLimitInvalidDecrease {
                parent_gas_limit: parent.header.gas_limit,
                child_gas_limit: child.header.gas_limit,
            })
        );
    }
}
