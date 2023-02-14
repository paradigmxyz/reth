//! Contains types that represent ethereum types in [reth_primitives] when used in RPC
use crate::Transaction;
use reth_primitives::{
    Address, Block as PrimitiveBlock, Bloom, Bytes, Header as RethHeader, H256, H64, U256,
};
use reth_rlp::Encodable;
use serde::{ser::Error, Deserialize, Serialize, Serializer};
use std::{collections::BTreeMap, ops::Deref};

/// Block Transactions depending on the boolean attribute of `eth_getBlockBy*`
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlockTransactions {
    /// Only hashes
    Hashes(Vec<H256>),
    /// Full transactions
    Full(Vec<Transaction>),
}

/// Error that can occur when converting other types to blocks
#[derive(Debug, thiserror::Error)]
pub enum BlockError {
    /// A transaction failed sender recovery
    #[error("transaction failed sender recovery")]
    InvalidSignature,
}

/// Block representation
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Header of the block
    #[serde(flatten)]
    pub header: Header,
    /// Total difficulty
    pub total_difficulty: U256,
    /// Uncles' hashes
    pub uncles: Vec<H256>,
    /// Transactions
    pub transactions: BlockTransactions,
    /// Size in bytes
    pub size: Option<U256>,
    /// Base Fee for post-EIP1559 blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_fee_per_gas: Option<U256>,
}

impl Block {
    /// Create a new block response from a [primitive block](reth_primitives::Block), using the
    /// total difficulty to populate its field in the rpc response.
    pub fn from_block_full(
        block: PrimitiveBlock,
        total_difficulty: U256,
    ) -> Result<Self, BlockError> {
        let block_hash = block.header.hash_slow();
        let header_length = block.header.length();
        let block_length = block.length();
        let uncles = block.ommers.into_iter().map(|h| h.hash_slow()).collect();

        let RethHeader {
            parent_hash,
            ommers_hash,
            beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            logs_bloom,
            difficulty,
            number,
            gas_limit,
            gas_used,
            timestamp,
            mix_hash,
            nonce,
            base_fee_per_gas,
            extra_data,
        } = block.header;

        let header = Header {
            hash: Some(block_hash),
            parent_hash,
            uncles_hash: ommers_hash,
            author: beneficiary,
            miner: beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            number: Some(U256::from(number)),
            gas_used: U256::from(gas_used),
            gas_limit: U256::from(gas_limit),
            extra_data,
            logs_bloom,
            timestamp: U256::from(timestamp),
            difficulty,
            mix_hash,
            nonce: Some(nonce.to_be_bytes().into()),
            size: Some(U256::from(header_length)),
        };

        let mut transactions = Vec::with_capacity(block.body.len());
        for (idx, tx) in block.body.iter().enumerate() {
            let signed_tx = tx.clone().into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
            transactions.push(Transaction::from_recovered_with_block_context(
                signed_tx,
                block_hash,
                number,
                U256::from(idx),
            ))
        }

        Ok(Self {
            header,
            uncles,
            transactions: BlockTransactions::Full(transactions),
            base_fee_per_gas: base_fee_per_gas.map(U256::from),
            total_difficulty,
            size: Some(U256::from(block_length)),
        })
    }
}

/// Block header representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Hash of the block
    pub hash: Option<H256>,
    /// Hash of the parent
    pub parent_hash: H256,
    /// Hash of the uncles
    #[serde(rename = "sha3Uncles")]
    pub uncles_hash: H256,
    /// Authors address
    pub author: Address,
    /// Alias of `author`
    pub miner: Address,
    /// State root hash
    pub state_root: H256,
    /// Transactions root hash
    pub transactions_root: H256,
    /// Transactions receipts root hash
    pub receipts_root: H256,
    /// Block number
    pub number: Option<U256>,
    /// Gas Used
    pub gas_used: U256,
    /// Gas Limit
    pub gas_limit: U256,
    /// Extra data
    pub extra_data: Bytes,
    /// Logs bloom
    pub logs_bloom: Bloom,
    /// Timestamp
    pub timestamp: U256,
    /// Difficulty
    pub difficulty: U256,
    /// Mix Hash
    pub mix_hash: H256,
    /// Nonce
    pub nonce: Option<H64>,
    /// Size in bytes
    pub size: Option<U256>,
}

/// A Block representation that allows to include additional fields
pub type RichBlock = Rich<Block>;

/// Header representation with additional info.
pub type RichHeader = Rich<Header>;

/// Value representation with additional info
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rich<T> {
    /// Standard value.
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
