//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Transaction wrapper that labels transaction with its origin.

use alloy_primitives::B256;
use alloy_rpc_types_eth::TransactionInfo;
use reth_primitives::{Recovered, TransactionSigned};
use reth_primitives_traits::SignedTransaction;
use reth_rpc_types_compat::TransactionCompat;

/// Represents from where a transaction was fetched.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransactionSource<T = TransactionSigned> {
    /// Transaction exists in the pool (Pending)
    Pool(Recovered<T>),
    /// Transaction already included in a block
    ///
    /// This can be a historical block or a pending block (received from the CL)
    Block {
        /// Transaction fetched via provider
        transaction: Recovered<T>,
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

impl<T: SignedTransaction> TransactionSource<T> {
    /// Consumes the type and returns the wrapped transaction.
    pub fn into_recovered(self) -> Recovered<T> {
        self.into()
    }

    /// Conversion into network specific transaction type.
    pub fn into_transaction<Builder: TransactionCompat<T>>(
        self,
        resp_builder: &Builder,
    ) -> Result<Builder::Transaction, Builder::Error> {
        match self {
            Self::Pool(tx) => resp_builder.fill_pending(tx),
            Self::Block { transaction, index, block_hash, block_number, base_fee } => {
                let tx_info = TransactionInfo {
                    hash: Some(transaction.trie_hash()),
                    index: Some(index),
                    block_hash: Some(block_hash),
                    block_number: Some(block_number),
                    base_fee: base_fee.map(u128::from),
                };

                resp_builder.fill(transaction, tx_info)
            }
        }
    }

    /// Returns the transaction and block related info, if not pending
    pub fn split(self) -> (Recovered<T>, TransactionInfo) {
        match self {
            Self::Pool(tx) => {
                let hash = tx.trie_hash();
                (tx, TransactionInfo { hash: Some(hash), ..Default::default() })
            }
            Self::Block { transaction, index, block_hash, block_number, base_fee } => {
                let hash = transaction.trie_hash();
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

impl<T> From<TransactionSource<T>> for Recovered<T> {
    fn from(value: TransactionSource<T>) -> Self {
        match value {
            TransactionSource::Pool(tx) => tx,
            TransactionSource::Block { transaction, .. } => transaction,
        }
    }
}
