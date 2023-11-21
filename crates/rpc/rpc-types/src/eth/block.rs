//! Contains types that represent ethereum types when used in RPC
use crate::{Transaction, Withdrawal};
use alloy_primitives::{Address, BlockHash, BlockNumber, Bloom, Bytes, B256, B64, U256, U64};
use alloy_rlp::{bytes, Decodable, Encodable, Error as RlpError};
use serde::{
    de::{MapAccess, Visitor},
    ser::{Error, SerializeStruct},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{
    collections::BTreeMap,
    fmt::{self, Formatter},
    num::ParseIntError,
    ops::Deref,
    str::FromStr,
};
/// Block Transactions depending on the boolean attribute of `eth_getBlockBy*`,
/// or if used by `eth_getUncle*`
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlockTransactions {
    /// Only hashes
    Hashes(Vec<B256>),
    /// Full transactions
    Full(Vec<Transaction>),
    /// Special case for uncle response.
    Uncle,
}

impl BlockTransactions {
    /// Check if the enum variant is
    /// used for an uncle response.
    pub fn is_uncle(&self) -> bool {
        matches!(self, Self::Uncle)
    }

    /// Returns an iterator over the transaction hashes.
    pub fn iter(&self) -> BlockTransactionsHashIterator<'_> {
        BlockTransactionsHashIterator::new(self)
    }
}

/// An Iterator over the transaction hashes of a block.
#[derive(Debug, Clone)]
pub struct BlockTransactionsHashIterator<'a> {
    txs: &'a BlockTransactions,
    idx: usize,
}

impl<'a> BlockTransactionsHashIterator<'a> {
    fn new(txs: &'a BlockTransactions) -> Self {
        Self { txs, idx: 0 }
    }
}

impl<'a> Iterator for BlockTransactionsHashIterator<'a> {
    type Item = B256;

    fn next(&mut self) -> Option<Self::Item> {
        match self.txs {
            BlockTransactions::Full(txs) => {
                let tx = txs.get(self.idx);
                self.idx += 1;
                tx.map(|tx| tx.hash)
            }
            BlockTransactions::Hashes(txs) => {
                let tx = txs.get(self.idx).copied();
                self.idx += 1;
                tx
            }
            BlockTransactions::Uncle => None,
        }
    }
}

/// Determines how the `transactions` field of [Block] should be filled.
///
/// This essentially represents the `full:bool` argument in RPC calls that determine whether the
/// response should include full transaction objects or just the hashes.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BlockTransactionsKind {
    /// Only include hashes: [BlockTransactions::Hashes]
    Hashes,
    /// Include full transaction objects: [BlockTransactions::Full]
    Full,
}

impl From<bool> for BlockTransactionsKind {
    fn from(is_full: bool) -> Self {
        if is_full {
            BlockTransactionsKind::Full
        } else {
            BlockTransactionsKind::Hashes
        }
    }
}

/// Error that can occur when converting other types to blocks
#[derive(Debug, thiserror::Error)]
pub enum BlockError {
    /// A transaction failed sender recovery
    #[error("transaction failed sender recovery")]
    InvalidSignature,
    /// A raw block failed to decode
    #[error("failed to decode raw block {0}")]
    RlpDecodeRawBlock(alloy_rlp::Error),
}

/// Block representation
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Header of the block
    #[serde(flatten)]
    pub header: Header,
    /// Total difficulty, this field is None only if representing
    /// an Uncle block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_difficulty: Option<U256>,
    /// Uncles' hashes
    pub uncles: Vec<B256>,
    /// Transactions
    #[serde(skip_serializing_if = "BlockTransactions::is_uncle")]
    pub transactions: BlockTransactions,
    /// Integer the size of this block in bytes.
    pub size: Option<U256>,
    /// Withdrawals in the block
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

/// Block header representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Hash of the block
    pub hash: Option<B256>,
    /// Hash of the parent
    pub parent_hash: B256,
    /// Hash of the uncles
    #[serde(rename = "sha3Uncles")]
    pub uncles_hash: B256,
    /// Alias of `author`
    pub miner: Address,
    /// State root hash
    pub state_root: B256,
    /// Transactions root hash
    pub transactions_root: B256,
    /// Transactions receipts root hash
    pub receipts_root: B256,
    /// Logs bloom
    pub logs_bloom: Bloom,
    /// Difficulty
    pub difficulty: U256,
    /// Block number
    pub number: Option<U256>,
    /// Gas Limit
    pub gas_limit: U256,
    /// Gas Used
    pub gas_used: U256,
    /// Timestamp
    pub timestamp: U256,
    /// Extra data
    pub extra_data: Bytes,
    /// Mix Hash
    pub mix_hash: B256,
    /// Nonce
    pub nonce: Option<B64>,
    /// Base fee per unit of gas (if past London)
    #[serde(rename = "baseFeePerGas", skip_serializing_if = "Option::is_none")]
    pub base_fee_per_gas: Option<U256>,
    /// Withdrawals root hash added by EIP-4895 and is ignored in legacy headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawals_root: Option<B256>,
    /// Blob gas used
    #[serde(rename = "blobGasUsed", skip_serializing_if = "Option::is_none")]
    pub blob_gas_used: Option<U64>,
    /// Excess blob gas
    #[serde(rename = "excessBlobGas", skip_serializing_if = "Option::is_none")]
    pub excess_blob_gas: Option<U64>,
    /// Parent beacon block root
    #[serde(rename = "parentBeaconBlockRoot", skip_serializing_if = "Option::is_none")]
    pub parent_beacon_block_root: Option<B256>,
}

/// A block hash which may have
/// a boolean requireCanonical field.
/// If false, an RPC call should raise if a block
/// matching the hash is not found.
/// If true, an RPC call should additionaly raise if
/// the block is not in the canonical chain.
/// <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1898.md#specification>
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RpcBlockHash {
    /// A block hash
    pub block_hash: B256,
    /// Whether the block must be a canonical block
    pub require_canonical: Option<bool>,
}

impl RpcBlockHash {
    /// Returns an [RpcBlockHash] from a [B256].
    pub const fn from_hash(block_hash: B256, require_canonical: Option<bool>) -> Self {
        RpcBlockHash { block_hash, require_canonical }
    }
}

impl From<B256> for RpcBlockHash {
    fn from(value: B256) -> Self {
        Self::from_hash(value, None)
    }
}

impl From<RpcBlockHash> for B256 {
    fn from(value: RpcBlockHash) -> Self {
        value.block_hash
    }
}

impl AsRef<B256> for RpcBlockHash {
    fn as_ref(&self) -> &B256 {
        &self.block_hash
    }
}

/// A block Number (or tag - "latest", "earliest", "pending")
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockNumberOrTag {
    /// Latest block
    #[default]
    Latest,
    /// Finalized block accepted as canonical
    Finalized,
    /// Safe head block
    Safe,
    /// Earliest block (genesis)
    Earliest,
    /// Pending block (not yet part of the blockchain)
    Pending,
    /// Block by number from canon chain
    Number(u64),
}

impl BlockNumberOrTag {
    /// Returns the numeric block number if explicitly set
    pub const fn as_number(&self) -> Option<u64> {
        match *self {
            BlockNumberOrTag::Number(num) => Some(num),
            _ => None,
        }
    }

    /// Returns `true` if a numeric block number is set
    pub const fn is_number(&self) -> bool {
        matches!(self, BlockNumberOrTag::Number(_))
    }

    /// Returns `true` if it's "latest"
    pub const fn is_latest(&self) -> bool {
        matches!(self, BlockNumberOrTag::Latest)
    }

    /// Returns `true` if it's "finalized"
    pub const fn is_finalized(&self) -> bool {
        matches!(self, BlockNumberOrTag::Finalized)
    }

    /// Returns `true` if it's "safe"
    pub const fn is_safe(&self) -> bool {
        matches!(self, BlockNumberOrTag::Safe)
    }

    /// Returns `true` if it's "pending"
    pub const fn is_pending(&self) -> bool {
        matches!(self, BlockNumberOrTag::Pending)
    }

    /// Returns `true` if it's "earliest"
    pub const fn is_earliest(&self) -> bool {
        matches!(self, BlockNumberOrTag::Earliest)
    }
}

impl From<u64> for BlockNumberOrTag {
    fn from(num: u64) -> Self {
        BlockNumberOrTag::Number(num)
    }
}

impl From<U64> for BlockNumberOrTag {
    fn from(num: U64) -> Self {
        num.to::<u64>().into()
    }
}

impl Serialize for BlockNumberOrTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            BlockNumberOrTag::Number(ref x) => serializer.serialize_str(&format!("0x{x:x}")),
            BlockNumberOrTag::Latest => serializer.serialize_str("latest"),
            BlockNumberOrTag::Finalized => serializer.serialize_str("finalized"),
            BlockNumberOrTag::Safe => serializer.serialize_str("safe"),
            BlockNumberOrTag::Earliest => serializer.serialize_str("earliest"),
            BlockNumberOrTag::Pending => serializer.serialize_str("pending"),
        }
    }
}

impl<'de> Deserialize<'de> for BlockNumberOrTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?.to_lowercase();
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for BlockNumberOrTag {
    type Err = ParseBlockNumberError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let block = match s {
            "latest" => Self::Latest,
            "finalized" => Self::Finalized,
            "safe" => Self::Safe,
            "earliest" => Self::Earliest,
            "pending" => Self::Pending,
            _number => {
                if let Some(hex_val) = s.strip_prefix("0x") {
                    let number = u64::from_str_radix(hex_val, 16);
                    BlockNumberOrTag::Number(number?)
                } else {
                    return Err(HexStringMissingPrefixError::default().into())
                }
            }
        };
        Ok(block)
    }
}

impl fmt::Display for BlockNumberOrTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockNumberOrTag::Number(ref x) => format!("0x{x:x}").fmt(f),
            BlockNumberOrTag::Latest => f.write_str("latest"),
            BlockNumberOrTag::Finalized => f.write_str("finalized"),
            BlockNumberOrTag::Safe => f.write_str("safe"),
            BlockNumberOrTag::Earliest => f.write_str("earliest"),
            BlockNumberOrTag::Pending => f.write_str("pending"),
        }
    }
}

/// Error variants when parsing a [BlockNumberOrTag]
#[derive(Debug, thiserror::Error)]
pub enum ParseBlockNumberError {
    /// Failed to parse hex value
    #[error(transparent)]
    ParseIntErr(#[from] ParseIntError),
    /// Block numbers should be 0x-prefixed
    #[error(transparent)]
    MissingPrefix(#[from] HexStringMissingPrefixError),
}

/// Thrown when a 0x-prefixed hex string was expected
#[derive(Copy, Clone, Debug, Default, thiserror::Error)]
#[non_exhaustive]
#[error("hex string without 0x prefix")]
pub struct HexStringMissingPrefixError;

/// A Block Identifier
/// <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1898.md>
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockId {
    /// A block hash and an optional bool that defines if it's canonical
    Hash(RpcBlockHash),
    /// A block number
    Number(BlockNumberOrTag),
}

// === impl BlockId ===

impl BlockId {
    /// Returns the block hash if it is [BlockId::Hash]
    pub const fn as_block_hash(&self) -> Option<B256> {
        match self {
            BlockId::Hash(hash) => Some(hash.block_hash),
            BlockId::Number(_) => None,
        }
    }

    /// Returns true if this is [BlockNumberOrTag::Latest]
    pub const fn is_latest(&self) -> bool {
        matches!(self, BlockId::Number(BlockNumberOrTag::Latest))
    }

    /// Returns true if this is [BlockNumberOrTag::Pending]
    pub const fn is_pending(&self) -> bool {
        matches!(self, BlockId::Number(BlockNumberOrTag::Pending))
    }
}

impl From<u64> for BlockId {
    fn from(num: u64) -> Self {
        BlockNumberOrTag::Number(num).into()
    }
}

impl From<U64> for BlockId {
    fn from(value: U64) -> Self {
        BlockNumberOrTag::Number(value.to()).into()
    }
}

impl From<BlockNumberOrTag> for BlockId {
    fn from(num: BlockNumberOrTag) -> Self {
        BlockId::Number(num)
    }
}

impl From<B256> for BlockId {
    fn from(block_hash: B256) -> Self {
        BlockId::Hash(RpcBlockHash { block_hash, require_canonical: None })
    }
}

impl From<(B256, Option<bool>)> for BlockId {
    fn from(hash_can: (B256, Option<bool>)) -> Self {
        BlockId::Hash(RpcBlockHash { block_hash: hash_can.0, require_canonical: hash_can.1 })
    }
}

impl Serialize for BlockId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            BlockId::Hash(RpcBlockHash { ref block_hash, ref require_canonical }) => {
                let mut s = serializer.serialize_struct("BlockIdEip1898", 1)?;
                s.serialize_field("blockHash", block_hash)?;
                if let Some(require_canonical) = require_canonical {
                    s.serialize_field("requireCanonical", require_canonical)?;
                }
                s.end()
            }
            BlockId::Number(ref num) => num.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BlockId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BlockIdVisitor;

        impl<'de> Visitor<'de> for BlockIdVisitor {
            type Value = BlockId;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("Block identifier following EIP-1898")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                // Since there is no way to clearly distinguish between a DATA parameter and a QUANTITY parameter. A str is therefor deserialized into a Block Number: <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1898.md>
                // However, since the hex string should be a QUANTITY, we can safely assume that if the len is 66 bytes, it is in fact a hash, ref <https://github.com/ethereum/go-ethereum/blob/ee530c0d5aa70d2c00ab5691a89ab431b73f8165/rpc/types.go#L184-L184>
                if v.len() == 66 {
                    Ok(BlockId::Hash(v.parse::<B256>().map_err(serde::de::Error::custom)?.into()))
                } else {
                    // quantity hex string or tag
                    Ok(BlockId::Number(v.parse().map_err(serde::de::Error::custom)?))
                }
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut number = None;
                let mut block_hash = None;
                let mut require_canonical = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "blockNumber" => {
                            if number.is_some() || block_hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("blockNumber"))
                            }
                            if require_canonical.is_some() {
                                return Err(serde::de::Error::custom(
                                    "Non-valid require_canonical field",
                                ))
                            }
                            number = Some(map.next_value::<BlockNumberOrTag>()?)
                        }
                        "blockHash" => {
                            if number.is_some() || block_hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("blockHash"))
                            }

                            block_hash = Some(map.next_value::<B256>()?);
                        }
                        "requireCanonical" => {
                            if number.is_some() || require_canonical.is_some() {
                                return Err(serde::de::Error::duplicate_field("requireCanonical"))
                            }

                            require_canonical = Some(map.next_value::<bool>()?)
                        }
                        key => {
                            return Err(serde::de::Error::unknown_field(
                                key,
                                &["blockNumber", "blockHash", "requireCanonical"],
                            ))
                        }
                    }
                }

                if let Some(number) = number {
                    Ok(BlockId::Number(number))
                } else if let Some(block_hash) = block_hash {
                    Ok(BlockId::Hash(RpcBlockHash { block_hash, require_canonical }))
                } else {
                    Err(serde::de::Error::custom(
                        "Expected `blockNumber` or `blockHash` with `requireCanonical` optionally",
                    ))
                }
            }
        }

        deserializer.deserialize_any(BlockIdVisitor)
    }
}

/// Block number and hash.
#[derive(Clone, Copy, Hash, Default, PartialEq, Eq)]
pub struct BlockNumHash {
    /// Block number
    pub number: BlockNumber,
    /// Block hash
    pub hash: BlockHash,
}

/// Block number and hash of the forked block.
pub type ForkBlock = BlockNumHash;

impl std::fmt::Debug for BlockNumHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("").field(&self.number).field(&self.hash).finish()
    }
}

impl BlockNumHash {
    /// Creates a new `BlockNumHash` from a block number and hash.
    pub fn new(number: BlockNumber, hash: BlockHash) -> Self {
        Self { number, hash }
    }

    /// Consumes `Self` and returns [`BlockNumber`], [`BlockHash`]
    pub fn into_components(self) -> (BlockNumber, BlockHash) {
        (self.number, self.hash)
    }

    /// Returns whether or not the block matches the given [BlockHashOrNumber].
    pub fn matches_block_or_num(&self, block: &BlockHashOrNumber) -> bool {
        match block {
            BlockHashOrNumber::Hash(hash) => self.hash == *hash,
            BlockHashOrNumber::Number(number) => self.number == *number,
        }
    }
}

impl From<(BlockNumber, BlockHash)> for BlockNumHash {
    fn from(val: (BlockNumber, BlockHash)) -> Self {
        BlockNumHash { number: val.0, hash: val.1 }
    }
}

impl From<(BlockHash, BlockNumber)> for BlockNumHash {
    fn from(val: (BlockHash, BlockNumber)) -> Self {
        BlockNumHash { hash: val.0, number: val.1 }
    }
}

/// Either a block hash _or_ a block number
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    any(test, feature = "arbitrary"),
    derive(proptest_derive::Arbitrary, arbitrary::Arbitrary)
)]
pub enum BlockHashOrNumber {
    /// A block hash
    Hash(B256),
    /// A block number
    Number(u64),
}

// === impl BlockHashOrNumber ===

impl BlockHashOrNumber {
    /// Returns the block number if it is a [`BlockHashOrNumber::Number`].
    #[inline]
    pub fn as_number(self) -> Option<u64> {
        match self {
            BlockHashOrNumber::Hash(_) => None,
            BlockHashOrNumber::Number(num) => Some(num),
        }
    }
}

impl From<B256> for BlockHashOrNumber {
    fn from(value: B256) -> Self {
        BlockHashOrNumber::Hash(value)
    }
}

impl From<u64> for BlockHashOrNumber {
    fn from(value: u64) -> Self {
        BlockHashOrNumber::Number(value)
    }
}

impl From<U64> for BlockHashOrNumber {
    fn from(value: U64) -> Self {
        value.to::<u64>().into()
    }
}

/// Allows for RLP encoding of either a block hash or block number
impl Encodable for BlockHashOrNumber {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match self {
            Self::Hash(block_hash) => block_hash.encode(out),
            Self::Number(block_number) => block_number.encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            Self::Hash(block_hash) => block_hash.length(),
            Self::Number(block_number) => block_number.length(),
        }
    }
}

/// Allows for RLP decoding of a block hash or block number
impl Decodable for BlockHashOrNumber {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header: u8 = *buf.first().ok_or(RlpError::InputTooShort)?;
        // if the byte string is exactly 32 bytes, decode it into a Hash
        // 0xa0 = 0x80 (start of string) + 0x20 (32, length of string)
        if header == 0xa0 {
            // strip the first byte, parsing the rest of the string.
            // If the rest of the string fails to decode into 32 bytes, we'll bubble up the
            // decoding error.
            let hash = B256::decode(buf)?;
            Ok(Self::Hash(hash))
        } else {
            // a block number when encoded as bytes ranges from 0 to any number of bytes - we're
            // going to accept numbers which fit in less than 64 bytes.
            // Any data larger than this which is not caught by the Hash decoding should error and
            // is considered an invalid block number.
            Ok(Self::Number(u64::decode(buf)?))
        }
    }
}

/// Error thrown when parsing a [BlockHashOrNumber] from a string.
#[derive(Debug, thiserror::Error)]
#[error("failed to parse {input:?} as a number: {parse_int_error} or hash: {hex_error}")]
pub struct ParseBlockHashOrNumberError {
    input: String,
    parse_int_error: ParseIntError,
    hex_error: alloy_primitives::hex::FromHexError,
}

impl FromStr for BlockHashOrNumber {
    type Err = ParseBlockHashOrNumberError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match u64::from_str(s) {
            Ok(val) => Ok(val.into()),
            Err(pares_int_error) => match B256::from_str(s) {
                Ok(val) => Ok(val.into()),
                Err(hex_error) => Err(ParseBlockHashOrNumberError {
                    input: s.to_string(),
                    parse_int_error: pares_int_error,
                    hex_error,
                }),
            },
        }
    }
}

/// A Block representation that allows to include additional fields
pub type RichBlock = Rich<Block>;

impl From<Block> for RichBlock {
    fn from(block: Block) -> Self {
        Rich { inner: block, extra_info: Default::default() }
    }
}

/// Header representation with additional info.
pub type RichHeader = Rich<Header>;

impl From<Header> for RichHeader {
    fn from(header: Header) -> Self {
        Rich { inner: header, extra_info: Default::default() }
    }
}

/// Value representation with additional info
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rich<T> {
    /// Standard value.
    #[serde(flatten)]
    pub inner: T,
    /// Additional fields that should be serialized into the `Block` object
    #[serde(flatten)]
    pub extra_info: BTreeMap<String, serde_json::Value>,
}

impl<T> Deref for Rich<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Serialize> Serialize for Rich<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.extra_info.is_empty() {
            return self.inner.serialize(serializer)
        }

        let inner = serde_json::to_value(&self.inner);
        let extras = serde_json::to_value(&self.extra_info);

        if let (Ok(serde_json::Value::Object(mut value)), Ok(serde_json::Value::Object(extras))) =
            (inner, extras)
        {
            value.extend(extras);
            value.serialize(serializer)
        } else {
            Err(S::Error::custom("Unserializable structures: expected objects"))
        }
    }
}

/// BlockOverrides is a set of header fields to override.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BlockOverrides {
    /// Overrides the block number.
    ///
    /// For `eth_callMany` this will be the block number of the first simulated block. Each
    /// following block increments its block number by 1
    // Note: geth uses `number`, erigon uses `blockNumber`
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "blockNumber")]
    pub number: Option<U256>,
    /// Overrides the difficulty of the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<U256>,
    /// Overrides the timestamp of the block.
    // Note: geth uses `time`, erigon uses `timestamp`
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timestamp")]
    pub time: Option<U64>,
    /// Overrides the gas limit of the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gas_limit: Option<U64>,
    /// Overrides the coinbase address of the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coinbase: Option<Address>,
    /// Overrides the prevrandao of the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random: Option<B256>,
    /// Overrides the basefee of the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_fee: Option<U256>,
    /// A dictionary that maps blockNumber to a user-defined hash. It could be queried from the
    /// solidity opcode BLOCKHASH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<BTreeMap<u64, B256>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_conversion() {
        let full = true;
        assert_eq!(BlockTransactionsKind::Full, full.into());

        let full = false;
        assert_eq!(BlockTransactionsKind::Hashes, full.into());
    }

    #[test]
    #[cfg(feature = "jsonrpsee-types")]
    fn serde_json_header() {
        use jsonrpsee_types::SubscriptionResponse;
        let resp = r#"{"jsonrpc":"2.0","method":"eth_subscribe","params":{"subscription":"0x7eef37ff35d471f8825b1c8f67a5d3c0","result":{"hash":"0x7a7ada12e140961a32395059597764416499f4178daf1917193fad7bd2cc6386","parentHash":"0xdedbd831f496e705e7f2ec3c8dcb79051040a360bf1455dbd7eb8ea6ad03b751","sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347","miner":"0x0000000000000000000000000000000000000000","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","transactionsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","receiptsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","number":"0x8","gasUsed":"0x0","gasLimit":"0x1c9c380","extraData":"0x","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","timestamp":"0x642aa48f","difficulty":"0x0","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","nonce":"0x0000000000000000"}}}"#;
        let _header: SubscriptionResponse<'_, Header> = serde_json::from_str(resp).unwrap();

        let resp = r#"{"jsonrpc":"2.0","method":"eth_subscription","params":{"subscription":"0x1a14b6bdcf4542fabf71c4abee244e47","result":{"author":"0x000000568b9b5a365eaa767d42e74ed88915c204","difficulty":"0x1","extraData":"0x4e65746865726d696e6420312e392e32322d302d6463373666616366612d32308639ad8ff3d850a261f3b26bc2a55e0f3a718de0dd040a19a4ce37e7b473f2d7481448a1e1fd8fb69260825377c0478393e6055f471a5cf839467ce919a6ad2700","gasLimit":"0x7a1200","gasUsed":"0x0","hash":"0xa4856602944fdfd18c528ef93cc52a681b38d766a7e39c27a47488c8461adcb0","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","miner":"0x0000000000000000000000000000000000000000","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","nonce":"0x0000000000000000","number":"0x434822","parentHash":"0x1a9bdc31fc785f8a95efeeb7ae58f40f6366b8e805f47447a52335c95f4ceb49","receiptsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347","size":"0x261","stateRoot":"0xf38c4bf2958e541ec6df148e54ce073dc6b610f8613147ede568cb7b5c2d81ee","totalDifficulty":"0x633ebd","timestamp":"0x604726b0","transactions":[],"transactionsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","uncles":[]}}}"#;
        let _header: SubscriptionResponse<'_, Header> = serde_json::from_str(resp).unwrap();
    }

    #[test]
    fn serde_block() {
        let block = Block {
            header: Header {
                hash: Some(B256::with_last_byte(1)),
                parent_hash: B256::with_last_byte(2),
                uncles_hash: B256::with_last_byte(3),
                miner: Address::with_last_byte(4),
                state_root: B256::with_last_byte(5),
                transactions_root: B256::with_last_byte(6),
                receipts_root: B256::with_last_byte(7),
                withdrawals_root: Some(B256::with_last_byte(8)),
                number: Some(U256::from(9)),
                gas_used: U256::from(10),
                gas_limit: U256::from(11),
                extra_data: Bytes::from(vec![1, 2, 3]),
                logs_bloom: Bloom::default(),
                timestamp: U256::from(12),
                difficulty: U256::from(13),
                mix_hash: B256::with_last_byte(14),
                nonce: Some(B64::with_last_byte(15)),
                base_fee_per_gas: Some(U256::from(20)),
                blob_gas_used: None,
                excess_blob_gas: None,
                parent_beacon_block_root: None,
            },
            total_difficulty: Some(U256::from(100000)),
            uncles: vec![B256::with_last_byte(17)],
            transactions: BlockTransactions::Hashes(vec![B256::with_last_byte(18)]),
            size: Some(U256::from(19)),
            withdrawals: Some(vec![]),
        };
        let serialized = serde_json::to_string(&block).unwrap();
        assert_eq!(
            serialized,
            r#"{"hash":"0x0000000000000000000000000000000000000000000000000000000000000001","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000002","sha3Uncles":"0x0000000000000000000000000000000000000000000000000000000000000003","miner":"0x0000000000000000000000000000000000000004","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000005","transactionsRoot":"0x0000000000000000000000000000000000000000000000000000000000000006","receiptsRoot":"0x0000000000000000000000000000000000000000000000000000000000000007","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","difficulty":"0xd","number":"0x9","gasLimit":"0xb","gasUsed":"0xa","timestamp":"0xc","extraData":"0x010203","mixHash":"0x000000000000000000000000000000000000000000000000000000000000000e","nonce":"0x000000000000000f","baseFeePerGas":"0x14","withdrawalsRoot":"0x0000000000000000000000000000000000000000000000000000000000000008","totalDifficulty":"0x186a0","uncles":["0x0000000000000000000000000000000000000000000000000000000000000011"],"transactions":["0x0000000000000000000000000000000000000000000000000000000000000012"],"size":"0x13","withdrawals":[]}"#
        );
        let deserialized: Block = serde_json::from_str(&serialized).unwrap();
        assert_eq!(block, deserialized);
    }

    #[test]
    fn serde_block_with_withdrawals_set_as_none() {
        let block = Block {
            header: Header {
                hash: Some(B256::with_last_byte(1)),
                parent_hash: B256::with_last_byte(2),
                uncles_hash: B256::with_last_byte(3),
                miner: Address::with_last_byte(4),
                state_root: B256::with_last_byte(5),
                transactions_root: B256::with_last_byte(6),
                receipts_root: B256::with_last_byte(7),
                withdrawals_root: None,
                number: Some(U256::from(9)),
                gas_used: U256::from(10),
                gas_limit: U256::from(11),
                extra_data: Bytes::from(vec![1, 2, 3]),
                logs_bloom: Bloom::default(),
                timestamp: U256::from(12),
                difficulty: U256::from(13),
                mix_hash: B256::with_last_byte(14),
                nonce: Some(B64::with_last_byte(15)),
                base_fee_per_gas: Some(U256::from(20)),
                blob_gas_used: None,
                excess_blob_gas: None,
                parent_beacon_block_root: None,
            },
            total_difficulty: Some(U256::from(100000)),
            uncles: vec![B256::with_last_byte(17)],
            transactions: BlockTransactions::Hashes(vec![B256::with_last_byte(18)]),
            size: Some(U256::from(19)),
            withdrawals: None,
        };
        let serialized = serde_json::to_string(&block).unwrap();
        assert_eq!(
            serialized,
            r#"{"hash":"0x0000000000000000000000000000000000000000000000000000000000000001","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000002","sha3Uncles":"0x0000000000000000000000000000000000000000000000000000000000000003","miner":"0x0000000000000000000000000000000000000004","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000005","transactionsRoot":"0x0000000000000000000000000000000000000000000000000000000000000006","receiptsRoot":"0x0000000000000000000000000000000000000000000000000000000000000007","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","difficulty":"0xd","number":"0x9","gasLimit":"0xb","gasUsed":"0xa","timestamp":"0xc","extraData":"0x010203","mixHash":"0x000000000000000000000000000000000000000000000000000000000000000e","nonce":"0x000000000000000f","baseFeePerGas":"0x14","totalDifficulty":"0x186a0","uncles":["0x0000000000000000000000000000000000000000000000000000000000000011"],"transactions":["0x0000000000000000000000000000000000000000000000000000000000000012"],"size":"0x13"}"#
        );
        let deserialized: Block = serde_json::from_str(&serialized).unwrap();
        assert_eq!(block, deserialized);
    }

    #[test]
    fn block_overrides() {
        let s = r#"{"blockNumber": "0xe39dd0"}"#;
        let _overrides = serde_json::from_str::<BlockOverrides>(s).unwrap();
    }

    #[test]
    fn serde_rich_block() {
        let s = r#"{
    "hash": "0xb25d0e54ca0104e3ebfb5a1dcdf9528140854d609886a300946fd6750dcb19f4",
    "parentHash": "0x9400ec9ef59689c157ac89eeed906f15ddd768f94e1575e0e27d37c241439a5d",
    "sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
    "miner": "0x829bd824b016326a401d083b33d092293333a830",
    "stateRoot": "0x546e330050c66d02923e7f1f3e925efaf64e4384eeecf2288f40088714a77a84",
    "transactionsRoot": "0xd5eb3ad6d7c7a4798cc5fb14a6820073f44a941107c5d79dac60bd16325631fe",
    "receiptsRoot": "0xb21c41cbb3439c5af25304e1405524c885e733b16203221900cb7f4b387b62f0",
    "logsBloom": "0x1f304e641097eafae088627298685d20202004a4a59e4d8900914724e2402b028c9d596660581f361240816e82d00fa14250c9ca89840887a381efa600288283d170010ab0b2a0694c81842c2482457e0eb77c2c02554614007f42aaf3b4dc15d006a83522c86a240c06d241013258d90540c3008888d576a02c10120808520a2221110f4805200302624d22092b2c0e94e849b1e1aa80bc4cc3206f00b249d0a603ee4310216850e47c8997a20aa81fe95040a49ca5a420464600e008351d161dc00d620970b6a801535c218d0b4116099292000c08001943a225d6485528828110645b8244625a182c1a88a41087e6d039b000a180d04300d0680700a15794",
    "difficulty": "0xc40faff9c737d",
    "number": "0xa9a230",
    "gasLimit": "0xbe5a66",
    "gasUsed": "0xbe0fcc",
    "timestamp": "0x5f93b749",
    "extraData": "0x7070796520e4b883e5bda9e7a59ee4bb99e9b1bc0103",
    "mixHash": "0xd5e2b7b71fbe4ddfe552fb2377bf7cddb16bbb7e185806036cee86994c6e97fc",
    "nonce": "0x4722f2acd35abe0f",
    "totalDifficulty": "0x3dc957fd8167fb2684a",
    "uncles": [],
    "transactions": [
        "0xf435a26acc2a9ef73ac0b73632e32e29bd0e28d5c4f46a7e18ed545c93315916"
    ],
    "size": "0xaeb6"
}"#;

        let block = serde_json::from_str::<RichBlock>(s).unwrap();
        let serialized = serde_json::to_string(&block).unwrap();
        let block2 = serde_json::from_str::<RichBlock>(&serialized).unwrap();
        assert_eq!(block, block2);
    }

    #[test]
    fn compact_block_number_serde() {
        let num: BlockNumberOrTag = 1u64.into();
        let serialized = serde_json::to_string(&num).unwrap();
        assert_eq!(serialized, "\"0x1\"");
    }
}
