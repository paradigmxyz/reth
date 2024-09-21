//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Transaction wrapper that labels transaction with its origin.

use alloy_primitives::B256;
use alloy_rpc_types::TransactionInfo;
use reth_primitives::TransactionSignedEcRecovered;
use reth_rpc_types_compat::{
    transaction::{from_recovered, from_recovered_with_block_context},
    TransactionCompat,
};

/// Represents from where a transaction was fetched.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransactionSource {
    /// Transaction exists in the pool (Pending)
    Pool(TransactionSignedEcRecovered),
    /// Transaction already included in a block
    ///
    /// This can be a historical block or a pending block (received from the CL)
    Block {
        /// Transaction fetched via provider
        transaction: TransactionSignedEcRecovered,
        /// Index of the transaction in the block
        index: u64,
        /// Hash of the block.
        block_hash: B256,
        /// Number of the block.
        block_number: u64,
        /// base fee of the block.
        base_fee: Option<u64>,
    },
}

// === impl TransactionSource ===

impl TransactionSource {
    /// Consumes the type and returns the wrapped transaction.
    pub fn into_recovered(self) -> TransactionSignedEcRecovered {
        self.into()
    }

    /// Conversion into network specific transaction type.
    pub fn into_transaction<T: TransactionCompat>(self) -> T::Transaction {
        match self {
            Self::Pool(tx) => from_recovered::<T>(tx),
            Self::Block { transaction, index, block_hash, block_number, base_fee } => {
                let tx_info = TransactionInfo {
                    hash: Some(transaction.hash()),
                    index: Some(index),
                    block_hash: Some(block_hash),
                    block_number: Some(block_number),
                    base_fee: base_fee.map(u128::from),
                };

                from_recovered_with_block_context::<T>(transaction, tx_info)
            }
        }
    }

    /// Returns the transaction and block related info, if not pending
    pub fn split(self) -> (TransactionSignedEcRecovered, TransactionInfo) {
        match self {
            Self::Pool(tx) => {
                let hash = tx.hash();
                (tx, TransactionInfo { hash: Some(hash), ..Default::default() })
            }
            Self::Block { transaction, index, block_hash, block_number, base_fee } => {
                let hash = transaction.hash();
                (
                    transaction,
                    TransactionInfo {
                        hash: Some(hash),
                        index: Some(index),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        base_fee: base_fee.map(u128::from),
                    },
                )
            }
        }
    }
}

impl From<TransactionSource> for TransactionSignedEcRecovered {
    fn from(value: TransactionSource) -> Self {
        match value {
            TransactionSource::Pool(tx) => tx,
            TransactionSource::Block { transaction, .. } => transaction,
        }
    }
}
