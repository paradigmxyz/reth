//! Contains types that represent ethereum types in [reth_primitives] when used in RPC
use crate::Transaction;
use reth_primitives::{
    Address, Block as PrimitiveBlock, Bloom, Bytes, Header as PrimitiveHeader, Withdrawal, H256,
    H64, U256,
};
use reth_rlp::Encodable;
use serde::{ser::Error, Deserialize, Serialize, Serializer};
use std::{collections::BTreeMap, ops::Deref};

/// Block Transactions depending on the boolean attribute of `eth_getBlockBy*`,
/// or if used by `eth_getUncle*`
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlockTransactions {
    /// Only hashes
    Hashes(Vec<H256>),
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
    /// Total difficulty, this field is None only if representing
    /// an Uncle block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_difficulty: Option<U256>,
    /// Uncles' hashes
    pub uncles: Vec<H256>,
    /// Transactions
    #[serde(skip_serializing_if = "BlockTransactions::is_uncle")]
    pub transactions: BlockTransactions,
    /// Integer the size of this block in bytes.
    pub size: Option<U256>,
    /// Base Fee for post-EIP1559 blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_fee_per_gas: Option<U256>,
    /// Withdrawals
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl Block {
    /// Converts the given primitive block into a [Block] response with the given
    /// [BlockTransactionsKind]
    ///
    /// If a `block_hash` is provided, then this is used, otherwise the block hash is computed.
    pub fn from_block(
        block: PrimitiveBlock,
        total_difficulty: U256,
        kind: BlockTransactionsKind,
        block_hash: Option<H256>,
    ) -> Result<Self, BlockError> {
        match kind {
            BlockTransactionsKind::Hashes => {
                Ok(Self::from_block_with_tx_hashes(block, total_difficulty, block_hash))
            }
            BlockTransactionsKind::Full => {
                Self::from_block_full(block, total_difficulty, block_hash)
            }
        }
    }

    /// Create a new [Block] response from a [primitive block](reth_primitives::Block), using the
    /// total difficulty to populate its field in the rpc response.
    ///
    /// This will populate the `transactions` field with only the hashes of the transactions in the
    /// block: [BlockTransactions::Hashes]
    pub fn from_block_with_tx_hashes(
        block: PrimitiveBlock,
        total_difficulty: U256,
        block_hash: Option<H256>,
    ) -> Self {
        let block_hash = block_hash.unwrap_or_else(|| block.header.hash_slow());
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
        block_hash: Option<H256>,
    ) -> Result<Self, BlockError> {
        let block_hash = block_hash.unwrap_or_else(|| block.header.hash_slow());
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
            total_difficulty: Some(total_difficulty),
            size: Some(U256::from(block_length)),
            withdrawals: block.withdrawals,
        }
    }

    /// Build an RPC block response representing
    /// an Uncle from its header.
    pub fn uncle_block_from_header(header: PrimitiveHeader) -> Self {
        let hash = header.hash_slow();
        let rpc_header = Header::from_primitive_with_hash(header.clone(), hash);
        let uncle_block = PrimitiveBlock { header, ..Default::default() };
        let size = Some(U256::from(uncle_block.length()));
        Self {
            uncles: vec![],
            header: rpc_header,
            transactions: BlockTransactions::Uncle,
            base_fee_per_gas: None,
            withdrawals: None,
            size,
            total_difficulty: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
}

// === impl Header ===

impl Header {
    /// Converts the primitive header type to this RPC type
    ///
    /// CAUTION: this takes the header's hash as is and does _not_ calculate the hash.
    pub fn from_primitive_with_hash(primitive_header: PrimitiveHeader, block_hash: H256) -> Self {
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

    #[test]
    fn serde_block() {
        let block = Block {
            header: Header {
                hash: Some(H256::from_low_u64_be(1)),
                parent_hash: H256::from_low_u64_be(2),
                uncles_hash: H256::from_low_u64_be(3),
                author: Address::from_low_u64_be(4),
                miner: Address::from_low_u64_be(4),
                state_root: H256::from_low_u64_be(5),
                transactions_root: H256::from_low_u64_be(6),
                receipts_root: H256::from_low_u64_be(7),
                withdrawals_root: Some(H256::from_low_u64_be(8)),
                number: Some(U256::from(9)),
                gas_used: U256::from(10),
                gas_limit: U256::from(11),
                extra_data: Bytes::from(vec![1, 2, 3]),
                logs_bloom: Bloom::default(),
                timestamp: U256::from(12),
                difficulty: U256::from(13),
                mix_hash: H256::from_low_u64_be(14),
                nonce: Some(H64::from_low_u64_be(15)),
            },
            total_difficulty: Some(U256::from(100000)),
            uncles: vec![H256::from_low_u64_be(17)],
            transactions: BlockTransactions::Hashes(vec![H256::from_low_u64_be(18)]),
            size: Some(U256::from(19)),
            base_fee_per_gas: Some(U256::from(20)),
            withdrawals: None,
        };
        let serialized = serde_json::to_string(&block).unwrap();
        assert_eq!(
            serialized,
            r#"{"hash":"0x0000000000000000000000000000000000000000000000000000000000000001","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000002","sha3Uncles":"0x0000000000000000000000000000000000000000000000000000000000000003","author":"0x0000000000000000000000000000000000000004","miner":"0x0000000000000000000000000000000000000004","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000005","transactionsRoot":"0x0000000000000000000000000000000000000000000000000000000000000006","receiptsRoot":"0x0000000000000000000000000000000000000000000000000000000000000007","withdrawalsRoot":"0x0000000000000000000000000000000000000000000000000000000000000008","number":"0x9","gasUsed":"0xa","gasLimit":"0xb","extraData":"0x010203","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","timestamp":"0xc","difficulty":"0xd","mixHash":"0x000000000000000000000000000000000000000000000000000000000000000e","nonce":"0x000000000000000f","totalDifficulty":"0x186a0","uncles":["0x0000000000000000000000000000000000000000000000000000000000000011"],"transactions":["0x0000000000000000000000000000000000000000000000000000000000000012"],"size":"0x13","baseFeePerGas":"0x14"}"#
        );
        let deserialized: Block = serde_json::from_str(&serialized).unwrap();
        assert_eq!(block, deserialized);
    }
}
