use crate::{error::PoolResult, validate::ValidPoolTransaction, BlockID};
use futures::channel::mpsc::Receiver;
use reth_primitives::{Address, TxHash, H256, U256};
use std::{fmt, sync::Arc};

/// General purpose abstraction fo a transaction-pool.
///
/// This is intended to be used by API-consumers such as RPC that need inject new incoming,
/// unverified transactions. And by block production that needs to get transactions to execute in a
/// new block.
#[async_trait::async_trait]
pub trait TransactionPool: Send + Sync {
    /// The transaction type of the pool
    type Transaction: PoolTransaction;

    /// Event listener for when a new block was mined.
    ///
    /// Implementers need to update the pool accordingly.
    /// For example the base fee of the pending block is determined after a block is mined which
    /// affects the dynamic fee requirement of pending transactions in the pool.
    async fn on_new_block(&self, event: NewBlockEvent);

    /// Adds an _unvalidated_ transaction into the pool.
    ///
    /// Consumer: RPC
    async fn add_transaction(
        &self,
        block_id: BlockID,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash>;

    /// Adds the given _unvalidated_ transaction into the pool.
    ///
    /// Returns a list of results.
    ///
    /// Consumer: RPC
    async fn add_transactions(
        &self,
        block_id: BlockID,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>>;

    /// Returns a new Stream that yields transactions hashes for new ready transactions.
    ///
    /// Consumer: RPC
    fn ready_transactions_listener(&self) -> Receiver<TxHash>;

    /// Returns an iterator that yields transactions that are ready for block production.
    ///
    /// Consumer: Block production
    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;

    /// Removes all transactions corresponding to the given hashes.
    ///
    /// Also removes all dependent transactions.
    ///
    /// Consumer: Block production
    fn remove_invalid(
        &self,
        tx_hashes: &[TxHash],
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns if the transaction for the given hash is already included in this pool.
    fn contains(&self, tx_hash: &TxHash) -> bool {
        self.get(tx_hash).is_some()
    }

    /// Returns the transaction for the given hash.
    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
}

/// Event fired when a new block was mined
#[derive(Debug, Clone)]
pub struct NewBlockEvent {
    /// Hash of the added block.
    pub hash: H256,
    /// EIP-1559 Base fee of the _next_ (pending) block
    ///
    /// The base fee of a block depends on the utilization of the last block and its base fee.
    pub pending_block_base_fee: U256,
    /// Provides a set of state changes that affected the accounts.
    // TODO based on the account changes, we can recheck balance
    pub state_changes: (),
}

/// An `Iterator` that only returns transactions that are ready to be executed.
///
/// This makes no assumptions about the order of the transactions, but expects that _all_
/// transactions are valid (no nonce gaps.).
pub trait BestTransactions: Iterator + Send {
    /// Mark the transaction as invalid.
    ///
    /// Implementers must ensure all subsequent transaction _don't_ depend on this transaction.
    /// In other words, this must remove the given transaction _and_ drain all transaction that
    /// depend on it.
    fn mark_invalid(&mut self, transaction: &Self::Item);
}

/// A no-op implementation that yields no transactions.
impl<T> BestTransactions for std::iter::Empty<T> {
    fn mark_invalid(&mut self, _tx: &T) {}
}

/// Trait for transaction types used inside the pool
pub trait PoolTransaction: fmt::Debug + Send + Sync + 'static {
    /// Hash of the transaction.
    fn hash(&self) -> &TxHash;

    /// The Sender of the transaction.
    fn sender(&self) -> &Address;

    /// Returns the nonce for this transaction.
    fn nonce(&self) -> u64;

    /// Calculates the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions that is `feeCap x gasLimit + transferred_value`
    fn cost(&self) -> U256;

    /// Returns the effective gas price for this transaction.
    ///
    /// This is `priority + basefee`for EIP-1559 and `gasPrice` for legacy transactions.
    fn effective_gas_price(&self) -> U256;

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
    fn gas_limit(&self) -> u64;

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_fee_per_gas(&self) -> Option<U256>;

    /// Returns the EIP-1559 Priority fee the caller is paying to the block author.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_priority_fee_per_gas(&self) -> Option<U256>;
}
