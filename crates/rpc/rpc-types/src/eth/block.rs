//! Contains types that represent ethereum types in [reth_primitives] when used in RPC
use crate::Transaction;
use reth_primitives::{
    Address, Block as PrimitiveBlock, Bloom, Bytes, Header as PrimitiveHeader, Withdrawal, H256,
    H64, U256,
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
    /// Withdrawals
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl Block {
    /// Converts the given primitive block into a [Block] response with the given
    /// [BlockTransactionsKind]
    pub fn from_block(
        block: PrimitiveBlock,
        total_difficulty: U256,
        kind: BlockTransactionsKind,
    ) -> Result<Self, BlockError> {
        match kind {
            BlockTransactionsKind::Hashes => {
                Ok(Self::from_block_hashes_only(block, total_difficulty))
            }
            BlockTransactionsKind::Full => Self::from_block_full(block, total_difficulty),
        }
    }

    /// Create a new [Block] response from a [primitive block](reth_primitives::Block), using the
    /// total difficulty to populate its field in the rpc response.
    ///
    /// This will populate the `transactions` field with only the hashes of the transactions in the
    /// block: [BlockTransactions::Hashes]
    pub fn from_block_hashes_only(block: PrimitiveBlock, total_difficulty: U256) -> Self {
        let block_hash = block.header.hash_slow();
        let transactions = block.body.iter().map(|tx| tx.hash).collect();

        Self::from_block_with_transactions(
            block_hash,
            block,
            total_difficulty,
            BlockTransactions::Hashes(transactions),
        )
    }

    /// Create a new [Block] response from a [primitive block](reth_primitives::Block), using the
    /// total difficulty to populate its field in the rpc response.
    ///
    /// This will populate the `transactions` field with the _full_ [Transaction] objects:
    /// [BlockTransactions::Full]
    pub fn from_block_full(
        block: PrimitiveBlock,
        total_difficulty: U256,
    ) -> Result<Self, BlockError> {
        let block_hash = block.header.hash_slow();
        let block_number = block.number;
        let mut transactions = Vec::with_capacity(block.body.len());
        for (idx, tx) in block.body.iter().enumerate() {
            let signed_tx = tx.clone().into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
            transactions.push(Transaction::from_recovered_with_block_context(
                signed_tx,
                block_hash,
                block_number,
                U256::from(idx),
            ))
        }

        Ok(Self::from_block_with_transactions(
            block_hash,
            block,
            total_difficulty,
            BlockTransactions::Full(transactions),
        ))
    }

    fn from_block_with_transactions(
        block_hash: H256,
        block: PrimitiveBlock,
        total_difficulty: U256,
        transactions: BlockTransactions,
    ) -> Self {
        let block_length = block.length();
        let uncles = block.ommers.into_iter().map(|h| h.hash_slow()).collect();
        let base_fee_per_gas = block.header.base_fee_per_gas;

        let header = Header::from_primitive_with_hash(block.header, block_hash);

        Self {
            header,
            uncles,
            transactions,
            base_fee_per_gas: base_fee_per_gas.map(U256::from),
            total_difficulty,
            size: Some(U256::from(block_length)),
            withdrawals: block.withdrawals,
        }
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
    /// Withdrawals root hash
    pub withdrawals_root: Option<H256>,
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

// === impl Header ===

impl Header {
    /// Converts the primitive header type to this RPC type
    ///
    /// CAUTION: this takes the header's hash as is and does _not_ calculate the hash.
    pub fn from_primitive_with_hash(primitive_header: PrimitiveHeader, block_hash: H256) -> Self {
        let header_length = primitive_header.length();

        let PrimitiveHeader {
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
            base_fee_per_gas: _,
            extra_data,
            withdrawals_root,
        } = primitive_header;

        Header {
            hash: Some(block_hash),
            parent_hash,
            uncles_hash: ommers_hash,
            author: beneficiary,
            miner: beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
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
}
