#![warn(missing_docs)]
#![deny(
    unused_must_use,
    rust_2018_idioms,
    unreachable_pub,
    missing_debug_implementations,
    rustdoc::broken_intra_doc_links,
    unused_crate_dependencies
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
    ordering::{CostOrdering, TransactionOrdering},
    traits::{
        BestTransactions, BlockInfo, CanonicalStateUpdate, ChangedAccount, PoolTransaction,
        PooledTransaction, PropagateKind, PropagatedTransactions, TransactionOrigin,
        TransactionPool,
    },
    validate::{
        EthTransactionValidator, TransactionValidationOutcome, TransactionValidator,
        ValidPoolTransaction,
    },
};
use crate::{
    error::{PoolError, PoolResult},
    pool::PoolInner,
    traits::{NewTransactionEvent, PoolSize},
};
use aquamarine as _;
use reth_primitives::{Address, TxHash, U256};
use reth_provider::StateProviderFactory;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc::Receiver;
use tracing::{instrument, trace};

mod config;
pub mod error;
mod identifier;
pub mod maintain;
pub mod metrics;
mod ordering;
pub mod pool;
mod traits;
mod validate;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking A pool
pub mod test_utils;

// TX_SLOT_SIZE is used to calculate how many data slots a single transaction
// takes up based on its size. The slots are used as DoS protection, ensuring
// that validating a new transaction remains a constant operation (in reality
// O(maxslots), where max slots are 4 currently).
pub(crate) const TX_SLOT_SIZE: usize = 32 * 1024;

// TX_MAX_SIZE is the maximum size a single transaction can have. This field has
// non-trivial consequences: larger transactions are significantly harder and
// more expensive to propagate; larger transactions also take more resources
// to validate whether they fit into the pool or not.
pub(crate) const TX_MAX_SIZE: usize = 4 * TX_SLOT_SIZE; //128KB

// Maximum bytecode to permit for a contract
pub(crate) const MAX_CODE_SIZE: usize = 24576;

// Maximum initcode to permit in a creation transaction and create instructions
pub(crate) const MAX_INIT_CODE_SIZE: usize = 2 * MAX_CODE_SIZE;

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
    pub fn new(validator: V, ordering: T, config: PoolConfig) -> Self {
        Self { pool: Arc::new(PoolInner::new(validator, ordering, config)) }
    }

    /// Returns the wrapped pool.
    pub(crate) fn inner(&self) -> &PoolInner<V, T> {
        &self.pool
    }

    /// Get the config the pool was configured with.
    pub fn config(&self) -> &PoolConfig {
        self.inner().config()
    }

    /// Sets the current block info for the pool.
    #[instrument(skip(self), target = "txpool")]
    pub fn set_block_info(&self, info: BlockInfo) {
        trace!(target: "txpool", "updating pool block info");
        self.pool.set_block_info(info)
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

impl<Client>
    Pool<EthTransactionValidator<Client, PooledTransaction>, CostOrdering<PooledTransaction>>
where
    Client: StateProviderFactory,
{
    /// Returns a new [Pool] that uses the default [EthTransactionValidator] when validating
    /// [PooledTransaction]s and ords via [CostOrdering]
    pub fn eth_pool(
        validator: EthTransactionValidator<Client, PooledTransaction>,
        config: PoolConfig,
    ) -> Self {
        Self::new(validator, CostOrdering::default(), config)
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

    fn pool_size(&self) -> PoolSize {
        self.pool.size()
    }

    fn block_info(&self) -> BlockInfo {
        self.pool.block_info()
    }

    fn on_canonical_state_change(&self, update: CanonicalStateUpdate) {
        self.pool.on_canonical_state_change(update);
    }

    async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash> {
        let (_, tx) = self.validate(origin, transaction).await;

        match tx {
            TransactionValidationOutcome::Valid { .. } => {
                self.pool.add_transactions(origin, std::iter::once(tx)).pop().expect("exists; qed")
            }
            TransactionValidationOutcome::Invalid(transaction, error) => {
                Err(PoolError::InvalidTransaction(*transaction.hash(), error))
            }
            TransactionValidationOutcome::Error(transaction, error) => {
                Err(PoolError::Other(*transaction.hash(), error))
            }
        }
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

    fn pooled_transaction_hashes(&self) -> Vec<TxHash> {
        self.pool.pooled_transactions_hashes()
    }

    fn pooled_transaction_hashes_max(&self, max: usize) -> Vec<TxHash> {
        self.pooled_transaction_hashes().into_iter().take(max).collect()
    }

    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pooled_transactions()
    }

    fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pooled_transactions().into_iter().take(max).collect()
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.best_transactions())
    }

    fn remove_transactions(
        &self,
        hashes: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_transactions(hashes)
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

    fn get_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_transactions_by_sender(sender)
    }
}

impl<V: TransactionValidator, T: TransactionOrdering> Clone for Pool<V, T> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}
