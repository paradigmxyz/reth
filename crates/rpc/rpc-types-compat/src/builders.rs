//! Unifies network specific RPC type conversions.

use std::fmt;

use reth_primitives::{BlockNumber, TransactionSignedEcRecovered, B256};

use crate::{BlockBuilder, TransactionBuilder};

/// Helper trait that unifies [`EthApiTypes`] with type conversions.
pub trait ResponseTypeBuilders: Send + Sync + Unpin + Clone + fmt::Debug {
    /// Builds RPC transaction response type w.r.t. network.
    type TransactionBuilder: TransactionBuilder;
    /// Builds RPC block response type w.r.t. network.
    type BlockBuilder: BlockBuilder<TxBuilder = Self::TransactionBuilder>;
}

impl<T> TransactionBuilder for T
where
    T: ResponseTypeBuilders,
{
    type Transaction = <T::TransactionBuilder as TransactionBuilder>::Transaction;

    fn fill(
        tx: TransactionSignedEcRecovered,
        block_hash: Option<B256>,
        block_number: Option<BlockNumber>,
        base_fee: Option<u64>,
        transaction_index: Option<usize>,
    ) -> Self::Transaction {
        T::TransactionBuilder::fill(tx, block_hash, block_number, base_fee, transaction_index)
    }
}

impl<T> BlockBuilder for T
where
    T: ResponseTypeBuilders,
{
    type TxBuilder = T::TransactionBuilder;
}
