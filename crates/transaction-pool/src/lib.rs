#![warn(missing_docs)]
// unreachable_pub, missing_debug_implementations
#![allow(unused)] // TODO(mattsse) remove after progress was made
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth's transaction pool implementation.
//!
//! This crate provides a generic transaction pool implementation.
//!
//! ## Functionality
//!
//! The transaction pool is responsible for
//!
//!      - recording incoming transactions
//!      - providing existing transactions
//!      - ordering and providing the best transactions for block production
//!      - monitoring memory footprint and enforce pool size limits
//!
//! ## Assumptions
//!
//! ### Transaction type
//!
//! The pool expects certain ethereum related information from the generic transaction type of the
//! pool ([`PoolTransaction`]), this includes gas price, base fee (EIP-1559 transactions), nonce
//! etc. It makes no assumptions about the encoding format, but the transaction type must report its
//! size so pool size limits (memory) can be enforced.
//!
//! ### Transaction ordering
//!
//! The pending pool contains transactions that can be mined on the current state.
//! The order in which they're returned are determined by a `Priority` value returned by the
//! `TransactionOrdering` type this pool is configured with.
//!
//! This is only used in the _pending_ pool to yield the best transactions for block production. The
//! _base pool_ is ordered by base fee, and the _queued pool_ by current distance.
//!
//! ### Validation
//!
//! The pool itself does not validate incoming transactions, instead this should be provided by
//! implementing `TransactionsValidator`. Only transactions that the validator returns as valid are
//! included in the pool. It is assumed that transaction that are in the pool are either valid on
//! the current state or could become valid after certain state changes. transaction that can never
//! become valid (e.g. nonce lower than current on chain nonce) will never be added to the pool and
//! instead are discarded right away.
//!
//! ### State Changes
//!
//! Once a new block is mined, the pool needs to be updated with a changeset in order to:
//!
//!     - remove mined transactions
//!     - update using account changes: balance changes
//!     - base fee updates
//!
//! ## Implementation details
//!
//! The `TransactionPool` trait exposes all externally used functionality of the pool, such as
//! inserting, querying specific transactions by hash or retrieving the best transactions.
//! Additionally, it allows to register event listeners for new ready transactions or state changes.
//! Events are communicated via channels.
//!
//! ### Architecture
//!
//! The final `TransactionPool` is made up of two layers:
//!
//! The lowest layer is the actual pool implementations that manages (validated) transactions:
//! [`TxPool`](crate::pool::TxPool). This is contained in a higher level pool type that guards the
//! low level pool and handles additional listeners or metrics:
//! [`PoolInner`](crate::pool::PoolInner)
//!
//! The transaction pool will be used by separate consumers (RPC, P2P), to make sharing easier, the
//! [`Pool`](crate::Pool) type is just an `Arc` wrapper around `PoolInner`. This is the usable type
//! that provides the `TransactionPool` interface.

pub use crate::{
    client::PoolClient,
    config::PoolConfig,
    ordering::TransactionOrdering,
    traits::{BestTransactions, NewBlockEvent, PoolTransaction, TransactionPool},
    validate::{TransactionValidationOutcome, TransactionValidator},
};
use crate::{
    error::PoolResult, pool::PoolInner, traits::NewTransactionEvent, validate::ValidPoolTransaction,
};
use futures::channel::mpsc::Receiver;
use reth_primitives::{BlockID, TxHash, U256, U64};
use std::{collections::HashMap, sync::Arc};

mod client;
mod config;
pub mod error;
mod identifier;
mod ordering;
pub mod pool;
mod traits;
mod validate;

#[cfg(test)]
mod test_util;

/// A shareable, generic, customizable `TransactionPool` implementation.
pub struct Pool<P: PoolClient, T: TransactionOrdering> {
    /// Arc'ed instance of the pool internals
    pool: Arc<PoolInner<P, T>>,
}

// === impl Pool ===

impl<P, T> Pool<P, T>
where
    P: PoolClient,
    T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
    /// Create a new transaction pool instance.
    pub fn new(client: Arc<P>, ordering: Arc<T>, config: PoolConfig) -> Self {
        Self { pool: Arc::new(PoolInner::new(client, ordering, config)) }
    }

    /// Returns the wrapped pool.
    pub(crate) fn inner(&self) -> &PoolInner<P, T> {
        &self.pool
    }

    /// Returns future that validates all transaction in the given iterator.
    async fn validate_all(
        &self,
        transactions: impl IntoIterator<Item = P::Transaction>,
    ) -> PoolResult<HashMap<TxHash, TransactionValidationOutcome<P::Transaction>>> {
        let outcome =
            futures::future::join_all(transactions.into_iter().map(|tx| self.validate(tx)))
                .await
                .into_iter()
                .collect::<HashMap<_, _>>();

        Ok(outcome)
    }

    /// Validates the given transaction
    async fn validate(
        &self,
        transaction: P::Transaction,
    ) -> (TxHash, TransactionValidationOutcome<P::Transaction>) {
        let hash = *transaction.hash();
        // TODO(mattsse): this is where additional validate checks would go, like banned senders
        // etc...
        let outcome = self.pool.client().validate_transaction(transaction).await;

        (hash, outcome)
    }

    /// Number of transactions in the entire pool
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// Whether the pool is empty
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
}

/// implements the `TransactionPool` interface for various transaction pool API consumers.
#[async_trait::async_trait]
impl<P, T> TransactionPool for Pool<P, T>
where
    P: PoolClient,
    T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
    type Transaction = T::Transaction;

    async fn on_new_block(&self, _event: NewBlockEvent) {
        // TODO perform maintenance: update pool accordingly
        todo!()
    }

    async fn add_transaction(&self, transaction: Self::Transaction) -> PoolResult<TxHash> {
        let (_, tx) = self.validate(transaction).await;
        self.pool.add_transactions(std::iter::once(tx)).pop().expect("exists; qed")
    }

    async fn add_transactions(
        &self,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        let validated = self.validate_all(transactions).await?;
        let transactions = self.pool.add_transactions(validated.into_values());
        Ok(transactions)
    }

    fn pending_transactions_listener(&self) -> Receiver<TxHash> {
        self.pool.add_pending_listener()
    }

    fn transactions_listener(&self) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        self.pool.add_transaction_listener()
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.ready_transactions())
    }

    fn remove_invalid(
        &self,
        _tx_hashes: &[TxHash],
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        todo!()
    }

    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner().get(tx_hash)
    }
}

impl<P: PoolClient, O: TransactionOrdering> Clone for Pool<P, O> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}
