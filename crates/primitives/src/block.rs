use crate::{Header, SealedHeader, TransactionSigned, Withdrawal, H256};
use ethers_core::types::BlockNumber;
use reth_codecs::derive_arbitrary;
use reth_rlp::{Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable};
use serde::{
    de::{MapAccess, Visitor},
    ser::SerializeStruct,
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{fmt, fmt::Formatter, ops::Deref, str::FromStr};

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[derive_arbitrary(rlp, 25)]
#[derive(
    Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, RlpEncodable, RlpDecodable,
)]
#[rlp(trailing)]
pub struct Block {
    /// Block header.
    pub header: Header,
    /// Transactions in this block.
    pub body: Vec<TransactionSigned>,
    /// Ommers/uncles header.
    pub ommers: Vec<Header>,
    /// Block withdrawals.
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl Deref for Block {
    type Target = Header;
    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

/// Sealed Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[derive_arbitrary(rlp, 10)]
#[derive(
    Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, RlpEncodable, RlpDecodable,
)]
#[rlp(trailing)]
pub struct SealedBlock {
    /// Locked block header.
    pub header: SealedHeader,
    /// Transactions with signatures.
    pub body: Vec<TransactionSigned>,
    /// Ommer/uncle headers
    pub ommers: Vec<SealedHeader>,
    /// Block withdrawals.
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl SealedBlock {
    /// Header hash.
    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    /// Splits the sealed block into underlying components
    pub fn split(self) -> (SealedHeader, Vec<TransactionSigned>, Vec<SealedHeader>) {
        (self.header, self.body, self.ommers)
    }

    /// Unseal the block
    pub fn unseal(self) -> Block {
        Block {
            header: self.header.unseal(),
            body: self.body,
            ommers: self.ommers.into_iter().map(|o| o.unseal()).collect(),
            withdrawals: self.withdrawals,
        }
    }
}

impl Deref for SealedBlock {
    type Target = SealedHeader;
    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

/// Either a block hash _or_ a block number
#[derive_arbitrary(rlp)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockHashOrNumber {
    /// A block hash
    Hash(H256),
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

impl From<H256> for BlockHashOrNumber {
    fn from(value: H256) -> Self {
        BlockHashOrNumber::Hash(value)
    }
}

impl From<u64> for BlockHashOrNumber {
    fn from(value: u64) -> Self {
        BlockHashOrNumber::Number(value)
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
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let header: u8 = *buf.first().ok_or(DecodeError::InputTooShort)?;
        // if the byte string is exactly 32 bytes, decode it into a Hash
        // 0xa0 = 0x80 (start of string) + 0x20 (32, length of string)
        if header == 0xa0 {
            // strip the first byte, parsing the rest of the string.
            // If the rest of the string fails to decode into 32 bytes, we'll bubble up the
            // decoding error.
            let hash = H256::decode(buf)?;
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
/// A Block Identifier
/// <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1898.md>
pub enum BlockId {
    /// A block hash and an optional bool that defines if it's canonical
    Hash(BlockHash),
    /// A block number
    Number(BlockNumberOrTag),
}

impl From<u64> for BlockId {
    fn from(num: u64) -> Self {
        BlockNumberOrTag::Number(num).into()
    }
}

impl From<BlockNumberOrTag> for BlockId {
    fn from(num: BlockNumberOrTag) -> Self {
        BlockId::Number(num)
    }
}

impl From<H256> for BlockId {
    fn from(block_hash: H256) -> Self {
        BlockId::Hash(BlockHash { block_hash, require_canonical: None })
    }
}

impl From<(H256, Option<bool>)> for BlockId {
    fn from(hash_can: (H256, Option<bool>)) -> Self {
        BlockId::Hash(BlockHash { block_hash: hash_can.0, require_canonical: hash_can.1 })
    }
}

impl From<BlockHash> for BlockId {
    fn from(hash_can: BlockHash) -> Self {
        BlockId::Hash(hash_can)
    }
}

impl From<BlockHashOrNumber> for BlockId {
    fn from(value: BlockHashOrNumber) -> Self {
        match value {
            BlockHashOrNumber::Hash(hash) => H256::from(hash.0).into(),
            BlockHashOrNumber::Number(number) => number.into(),
        }
    }
}

impl Serialize for BlockId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            BlockId::Hash(BlockHash { ref block_hash, ref require_canonical }) => {
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
                Ok(BlockId::Number(v.parse().map_err(serde::de::Error::custom)?))
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

                            block_hash = Some(map.next_value::<H256>()?);
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
                    Ok(BlockId::Hash(BlockHash { block_hash, require_canonical }))
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
    pub fn as_number(&self) -> Option<u64> {
        match *self {
            BlockNumberOrTag::Number(num) => Some(num),
            _ => None,
        }
    }

    /// Returns `true` if a numeric block number is set
    pub fn is_number(&self) -> bool {
        matches!(self, BlockNumberOrTag::Number(_))
    }

    /// Returns `true` if it's "latest"
    pub fn is_latest(&self) -> bool {
        matches!(self, BlockNumberOrTag::Latest)
    }

    /// Returns `true` if it's "finalized"
    pub fn is_finalized(&self) -> bool {
        matches!(self, BlockNumberOrTag::Finalized)
    }

    /// Returns `true` if it's "safe"
    pub fn is_safe(&self) -> bool {
        matches!(self, BlockNumberOrTag::Safe)
    }

    /// Returns `true` if it's "pending"
    pub fn is_pending(&self) -> bool {
        matches!(self, BlockNumberOrTag::Pending)
    }

    /// Returns `true` if it's "earliest"
    pub fn is_earliest(&self) -> bool {
        matches!(self, BlockNumberOrTag::Earliest)
    }
}

impl From<u64> for BlockNumberOrTag {
    fn from(num: u64) -> Self {
        BlockNumberOrTag::Number(num)
    }
}

impl From<ethers_core::types::BlockNumber> for BlockNumberOrTag {
    fn from(value: BlockNumber) -> Self {
        match value {
            BlockNumber::Latest => BlockNumberOrTag::Latest,
            BlockNumber::Finalized => BlockNumberOrTag::Finalized,
            BlockNumber::Safe => BlockNumberOrTag::Safe,
            BlockNumber::Earliest => BlockNumberOrTag::Earliest,
            BlockNumber::Pending => BlockNumberOrTag::Pending,
            BlockNumber::Number(num) => BlockNumberOrTag::Number(num.as_u64()),
        }
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
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let block = match s {
            "latest" => Self::Latest,
            "finalized" => Self::Finalized,
            "safe" => Self::Safe,
            "earliest" => Self::Earliest,
            "pending" => Self::Pending,
            _number => {
                let hex_string = s.trim_start_matches("0x");
                let number = u64::from_str_radix(hex_string, 16).map_err(|err| err.to_string());
                BlockNumberOrTag::Number(number?)
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

/// A block hash which may have
/// a boolean requireCanonical field.
/// If false, an RPC call should raise if a block
/// matching the hash is not found.
/// If true, an RPC call should additionaly raise if
/// the block is not in the canonical chain.
/// <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1898.md#specification>
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockHash {
    /// A block hash
    pub block_hash: H256,
    /// Whether the block must be a canonical block
    pub require_canonical: Option<bool>,
}

impl BlockHash {
    pub fn from_hash(block_hash: H256, require_canonical: Option<bool>) -> Self {
        BlockHash { block_hash, require_canonical }
    }
}

impl From<H256> for BlockHash {
    fn from(value: H256) -> Self {
        Self::from_hash(value, None)
    }
}

impl From<BlockHash> for H256 {
    fn from(value: BlockHash) -> Self {
        value.block_hash
    }
}

impl AsRef<H256> for BlockHash {
    fn as_ref(&self) -> &H256 {
        &self.block_hash
    }
}

#[cfg(test)]
mod test {
    use super::{BlockId, BlockNumberOrTag::*, *};
    /// Check parsing according to EIP-1898.
    #[test]
    fn can_parse_blockid_u64() {
        let num = serde_json::json!(
            {"blockNumber": "0xaf"}
        );

        let id = serde_json::from_value::<BlockId>(num);
        assert_eq!(id.unwrap(), BlockId::from(175));
    }
    #[test]
    fn can_parse_block_hash() {
        let block_hash =
            H256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        let block_hash_json = serde_json::json!(
            { "blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}
        );
        let id = serde_json::from_value::<BlockId>(block_hash_json).unwrap();
        assert_eq!(id, BlockId::from(block_hash,));
    }
    #[test]
    fn can_parse_block_hash_with_canonical() {
        let block_hash =
            H256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        let block_id = BlockId::Hash(BlockHash::from_hash(block_hash, Some(true)));
        let block_hash_json = serde_json::json!(
            { "blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3", "requireCanonical": true }
        );
        let id = serde_json::from_value::<BlockId>(block_hash_json).unwrap();
        assert_eq!(id, block_id)
    }
    #[test]
    fn can_parse_blockid_tags() {
        let tags =
            [("latest", Latest), ("finalized", Finalized), ("safe", Safe), ("pending", Pending)];
        for (value, tag) in tags {
            let num = serde_json::json!({ "blockNumber": value });
            let id = serde_json::from_value::<BlockId>(num);
            assert_eq!(id.unwrap(), BlockId::from(tag))
        }
    }
    #[test]
    fn repeated_keys_is_err() {
        let num = serde_json::json!({"blockNumber": 1, "requireCanonical": true, "requireCanonical": false});
        assert!(serde_json::from_value::<BlockId>(num).is_err());
        let num =
            serde_json::json!({"blockNumber": 1, "requireCanonical": true, "blockNumber": 23});
        assert!(serde_json::from_value::<BlockId>(num).is_err());
    }
    /// Serde tests
    #[test]
    fn serde_blockid_tags() {
        let block_ids = [Latest, Finalized, Safe, Pending].map(BlockId::from);
        for block_id in &block_ids {
            let serialized = serde_json::to_string(&block_id).unwrap();
            let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, *block_id)
        }
    }
    #[test]
    fn serde_blockid_number() {
        let block_id = BlockId::from(100u64);
        let serialized = serde_json::to_string(&block_id).unwrap();
        let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, block_id)
    }
    #[test]
    fn serde_blockid_hash() {
        let block_id = BlockId::from(H256::default());
        let serialized = serde_json::to_string(&block_id).unwrap();
        let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, block_id)
    }
    #[test]
    fn serde_rpc_payload_block_tag() {
        let payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},"latest"],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap();
        let block_id: BlockId = serde_json::from_value::<BlockId>(block_id_param.clone()).unwrap();
        assert_eq!(BlockId::Number(BlockNumberOrTag::Latest), block_id);
    }
    #[test]
    fn serde_rpc_payload_block_object() {
        let example_payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},{"blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(example_payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap().to_string();
        let block_id: BlockId = serde_json::from_str::<BlockId>(&block_id_param).unwrap();
        let hash =
            H256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        assert_eq!(BlockId::from(hash), block_id);
        let serialized = serde_json::to_string(&BlockId::from(hash)).unwrap();
        assert_eq!("{\"blockHash\":\"0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3\"}", serialized)
    }
    #[test]
    fn serde_rpc_payload_block_number() {
        let example_payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},{"blockNumber": "0x0"}],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(example_payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap().to_string();
        let block_id: BlockId = serde_json::from_str::<BlockId>(&block_id_param).unwrap();
        assert_eq!(BlockId::from(0u64), block_id);
        let serialized = serde_json::to_string(&BlockId::from(0u64)).unwrap();
        assert_eq!("\"0x0\"", serialized)
    }
    #[test]
    #[should_panic]
    fn serde_rpc_payload_block_number_duplicate_key() {
        let payload = r#"{"blockNumber": "0x132", "blockNumber": "0x133"}"#;
        let parsed_block_id = serde_json::from_str::<BlockId>(payload);
        parsed_block_id.unwrap();
    }
    #[test]
    fn serde_rpc_payload_block_hash() {
        let payload = r#"{"blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}"#;
        let parsed = serde_json::from_str::<BlockId>(payload).unwrap();
        let expected = BlockId::from(
            H256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap(),
        );
        assert_eq!(parsed, expected);
    }
}
