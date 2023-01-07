#![warn(missing_docs)]
#![deny(
    unused_must_use,
    rust_2018_idioms,
    unreachable_pub,
    missing_debug_implementations,
    rustdoc::broken_intra_doc_links
)]
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
//!    - recording incoming transactions
//!    - providing existing transactions
//!    - ordering and providing the best transactions for block production
//!    - monitoring memory footprint and enforce pool size limits
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
//!   - remove mined transactions
//!   - update using account changes: balance changes
//!   - base fee updates
//!
//! ## Implementation details
//!
//! The `TransactionPool` trait exposes all externally used functionality of the pool, such as
//! inserting, querying specific transactions by hash or retrieving the best transactions.
//! In addition, it enables the registration of event listeners that are notified of state changes.
//! Events are communicated via channels.
//!
//! ### Architecture
//!
//! The final `TransactionPool` is made up of two layers:
//!
//! The lowest layer is the actual pool implementations that manages (validated) transactions:
//! [`TxPool`](crate::pool::txpool::TxPool). This is contained in a higher level pool type that
//! guards the low level pool and handles additional listeners or metrics:
//! [`PoolInner`](crate::pool::PoolInner)
//!
//! The transaction pool will be used by separate consumers (RPC, P2P), to make sharing easier, the
//! [`Pool`](crate::Pool) type is just an `Arc` wrapper around `PoolInner`. This is the usable type
//! that provides the `TransactionPool` interface.

pub use crate::{
    config::PoolConfig,
    ordering::TransactionOrdering,
    traits::{
        BestTransactions, OnNewBlockEvent, PoolTransaction, PropagateKind, PropagatedTransactions,
        TransactionOrigin, TransactionPool,
    },
    validate::{TransactionValidationOutcome, TransactionValidator},
};
use crate::{
    error::PoolResult,
    pool::PoolInner,
    traits::{NewTransactionEvent, PoolSize},
    validate::ValidPoolTransaction,
};
use reth_primitives::{TxHash, U256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc::Receiver;

mod config;
pub mod error;
mod identifier;
pub mod metrics;
mod ordering;
pub mod pool;
mod traits;
mod validate;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking A pool
pub mod test_utils;

/// A shareable, generic, customizable `TransactionPool` implementation.
#[derive(Debug)]
pub struct Pool<V: TransactionValidator, T: TransactionOrdering> {
    /// Arc'ed instance of the pool internals
    pool: Arc<PoolInner<V, T>>,
}

// === impl Pool ===

impl<V, T> Pool<V, T>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
{
    /// Create a new transaction pool instance.
    pub fn new(client: Arc<V>, ordering: Arc<T>, config: PoolConfig) -> Self {
        Self { pool: Arc::new(PoolInner::new(client, ordering, config)) }
    }

    /// Returns the wrapped pool.
    pub(crate) fn inner(&self) -> &PoolInner<V, T> {
        &self.pool
    }

    /// Get the config the pool was configured with.
    pub fn config(&self) -> &PoolConfig {
        self.inner().config()
    }

    /// Returns future that validates all transaction in the given iterator.
    async fn validate_all(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = V::Transaction>,
    ) -> PoolResult<HashMap<TxHash, TransactionValidationOutcome<V::Transaction>>> {
        let outcome = futures_util::future::join_all(
            transactions.into_iter().map(|tx| self.validate(origin, tx)),
        )
        .await
        .into_iter()
        .collect::<HashMap<_, _>>();

        Ok(outcome)
    }

    /// Validates the given transaction
    async fn validate(
        &self,
        origin: TransactionOrigin,
        transaction: V::Transaction,
    ) -> (TxHash, TransactionValidationOutcome<V::Transaction>) {
        let hash = *transaction.hash();
        // TODO(mattsse): this is where additional validate checks would go, like banned senders
        // etc...
        let outcome = self.pool.validator().validate_transaction(origin, transaction).await;

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
impl<V, T> TransactionPool for Pool<V, T>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
{
    type Transaction = T::Transaction;

    fn status(&self) -> PoolSize {
        self.pool.size()
    }

    fn on_new_block(&self, event: OnNewBlockEvent) {
        self.pool.on_new_block(event);
    }

    async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash> {
        let (_, tx) = self.validate(origin, transaction).await;
        self.pool.add_transactions(origin, std::iter::once(tx)).pop().expect("exists; qed")
    }

    async fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        let validated = self.validate_all(origin, transactions).await?;
        let transactions = self.pool.add_transactions(origin, validated.into_values());
        Ok(transactions)
    }

    fn pending_transactions_listener(&self) -> Receiver<TxHash> {
        self.pool.add_pending_listener()
    }

    fn transactions_listener(&self) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        self.pool.add_transaction_listener()
    }

    fn pooled_transactions(&self) -> Vec<TxHash> {
        self.pool.pooled_transactions()
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.best_transactions())
    }

    fn remove_invalid(
        &self,
        hashes: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_invalid(hashes)
    }

    fn retain_unknown(&self, hashes: &mut Vec<TxHash>) {
        self.pool.retain_unknown(hashes)
    }

    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner().get(tx_hash)
    }

    fn get_all(
        &self,
        txs: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner().get_all(txs)
    }

    fn on_propagated(&self, txs: PropagatedTransactions) {
        self.inner().on_propagated(txs)
    }
}

impl<V: TransactionValidator, O: TransactionOrdering> Clone for Pool<V, O> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}
