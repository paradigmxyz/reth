use crate::{error::PoolResult, validate::ValidPoolTransaction, BlockId};
use reth_primitives::{U256, U64};
use std::{
    fmt,
    hash::Hash,
    sync::{mpsc::Receiver, Arc},
};

pub type HashFor<T> = <<T as TransactionPool>::Transaction as PoolTransaction>::Hash;

/// General purpose abstraction fo a transaction-pool.
///
/// This is intended to be used by API-consumers such as RPC that need inject new incoming,
/// unverified transactions. And by block production that needs to get transactions to execute in a
/// new block.
#[async_trait::async_trait]
pub trait TransactionPool: Send + Sync {
    /// The transaction type of the pool
    type Transaction: PoolTransaction + Send + Sync;

    /// Adds an unvalidated transaction into the pool.
    ///
    /// Consumer: RPC
    async fn add_transaction(
        &self,
        block_id: &BlockId,
        transaction: Self::Transaction,
    ) -> PoolResult<HashFor<Self>>;

    /// Adds all unvalidated transaction into the pool.
    ///
    /// Returns a list of results.
    ///
    /// Consumer: RPC
    async fn add_transactions(
        &self,
        block_id: &BlockId,
        transaction: Self::Transaction,
    ) -> PoolResult<Vec<PoolResult<HashFor<Self>>>>;

    /// Returns a new Stream that yields transactions hashes for new ready transactions.
    ///
    /// Consumer: RPC
    fn ready_transactions(&self) -> Receiver<HashFor<Self>>;

    /// Returns an iterator that yields transactions that are ready for block production.
    ///
    /// This provides the block at which the pool should be updated at.
    ///
    /// Implementers must ensure that the iterator yields only transaction that are valid for the
    /// given `block` and return `None` otherwise.
    ///
    /// Consumer: Block production
    async fn ready_transactions_at(
        &self,
        block: U64,
    ) -> Box<dyn ReadyTransactions<Item = ValidPoolTransaction<Self::Transaction>>>;

    /// Removes all transactions corresponding to the given hashes.
    ///
    /// Also removes all dependent transactions.
    ///
    /// Consumer: Block production
    fn remove_invalid(
        &self,
        tx_hashes: &[HashFor<Self>],
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
}

/// An `Iterator` that only returns transactions that are ready to be executed.
///
/// This makes no assumptions about the order of the transactions, but expects that _all_
/// transactions are valid (no nonce gaps.).
pub trait ReadyTransactions: Iterator + Send {
    /// Mark the transaction as invalid.
    ///
    /// Implementers must ensure all subsequent transaction _don't_ depend on this transaction.
    /// In other words, this must remove the given transaction _and_ drain all transaction that
    /// depend on it.
    fn mark_invalid(&mut self, transaction: &Self::Item);
}

/// A no-op implementation that yields no transactions.
impl<T> ReadyTransactions for std::iter::Empty<T> {
    fn mark_invalid(&mut self, _tx: &T) {}
}

/// Trait for transaction types used inside the pool
pub trait PoolTransaction: fmt::Debug + Send + Send {
    /// Transaction hash type.
    type Hash: fmt::Debug + fmt::LowerHex + Eq + Clone + Copy + Hash + Send + Sync + 'static;

    /// Unique identifier for this transaction.
    type Id: fmt::Debug + fmt::LowerHex + Eq + Clone + Hash + AsRef<Self::Id> + Send + Sync;

    /// Transaction sender type.
    type Sender: fmt::Debug + Eq + Clone + Hash + Send + Sync;

    /// Hash of the transaction
    fn hash(&self) -> &Self::Hash;

    /// The Sender of the transaction
    fn sender(&self) -> &Self::Sender;

    /// Creates the unique identifier for this transaction.
    fn id(&self) -> Self::Id;

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_fee_per_gas(&self) -> Option<&U256>;

    /// Returns the EIP-1559 Priority fee the caller is paying to the block author.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_priority_fee_per_gas(&self) -> Option<&U256>;
}
