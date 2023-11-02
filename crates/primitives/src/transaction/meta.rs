use crate::B256;

/// Additional fields in the context of a block that contains this transaction.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct TransactionMeta {
    /// Hash of the transaction.
    pub tx_hash: B256,
    /// Index of the transaction in the block
    pub index: u64,
    /// Hash of the block.
    pub block_hash: B256,
    /// Number of the block.
    pub block_number: u64,
    /// Base fee of the block.
    pub base_fee: Option<u64>,
    /// The excess blob gas of the block.
    pub excess_blob_gas: Option<u64>,
}
