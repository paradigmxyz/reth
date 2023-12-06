//! Commonly used additional types that are not part of the JSON RPC spec but are often required
//! when working with RPC types, such as [Transaction](crate::Transaction)

use alloy_primitives::{TxHash, B256};

/// Additional fields in the context of a block that contains this transaction.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct TransactionInfo {
    /// Hash of the transaction.
    pub hash: Option<TxHash>,
    /// Index of the transaction in the block
    pub index: Option<u64>,
    /// Hash of the block.
    pub block_hash: Option<B256>,
    /// Number of the block.
    pub block_number: Option<u64>,
    /// Base fee of the block.
    pub base_fee: Option<u64>,
}
