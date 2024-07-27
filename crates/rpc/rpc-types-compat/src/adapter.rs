use std::{fmt, marker::PhantomData};

use derive_more::Constructor;
use reth_primitives::{BlockNumber, TransactionSignedEcRecovered, B256};

use crate::{BlockBuilder, TransactionBuilder};

/// Adapter for builders of network specific RPC types.
#[derive(Debug, Constructor)]
pub struct NetworkTypeBuilders<T, B> {
    tx_builder: T,
    _phantom: PhantomData<B>,
}

impl<T, B> TransactionBuilder for NetworkTypeBuilders<T, B>
where
    T: TransactionBuilder,
    B: Send + Sync + Unpin + fmt::Debug,
{
    type Transaction = T::Transaction;

    fn fill(
        &self,
        tx: TransactionSignedEcRecovered,
        block_hash: Option<B256>,
        block_number: Option<BlockNumber>,
        base_fee: Option<u64>,
        transaction_index: Option<usize>,
    ) -> Self::Transaction {
        self.tx_builder.fill(tx, block_hash, block_number, base_fee, transaction_index)
    }
}

impl<T, B> BlockBuilder for NetworkTypeBuilders<T, B>
where
    T: TransactionBuilder,
    B: BlockBuilder<Transaction = T::Transaction>,
{
    type Transaction = B::Transaction;

    fn tx_builder(&self) -> impl TransactionBuilder<Transaction = Self::Transaction> {
        &self.tx_builder
    }
}
