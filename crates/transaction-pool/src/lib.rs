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
//!    - storing blob data for transactions in a separate blobstore on insertion
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
//! guards the low level pool and handles additional listeners or metrics: [`PoolInner`].
//!
//! The transaction pool will be used by separate consumers (RPC, P2P), to make sharing easier, the
//! [`Pool`] type is just an `Arc` wrapper around `PoolInner`. This is the usable type that provides
//! the `TransactionPool` interface.
//!
//!
//! ## Blob Transactions
//!
//! Blob transaction can be quite large hence they are stored in a separate blobstore. The pool is
//! responsible for inserting blob data for new transactions into the blobstore.
//! See also [ValidTransaction](validate::ValidTransaction)
//!
//!
//! ## Examples
//!
//! Listen for new transactions and print them:
//!
//! ```
//! use reth_primitives::MAINNET;
//! use reth_provider::{BlockReaderIdExt, ChainSpecProvider, StateProviderFactory};
//! use reth_tasks::TokioTaskExecutor;
//! use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool, TransactionPool};
//! use reth_transaction_pool::blobstore::InMemoryBlobStore;
//! async fn t<C>(client: C)  where C: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static{
//!     let blob_store = InMemoryBlobStore::default();
//!     let pool = Pool::eth_pool(
//!         TransactionValidationTaskExecutor::eth(client, MAINNET.clone(), blob_store.clone(), TokioTaskExecutor::default()),
//!         blob_store,
//!         Default::default(),
//!     );
//!   let mut transactions = pool.pending_transactions_listener();
//!   tokio::task::spawn( async move {
//!      while let Some(tx) = transactions.recv().await {
//!          println!("New transaction: {:?}", tx);
//!      }
//!   });
//!
//!   // do something useful with the pool, like RPC integration
//!
//! # }
//! ```
//!
//! Spawn maintenance task to keep the pool updated
//!
//! ```
//! use futures_util::Stream;
//! use reth_primitives::MAINNET;
//! use reth_provider::{BlockReaderIdExt, CanonStateNotification, ChainSpecProvider, StateProviderFactory};
//! use reth_tasks::TokioTaskExecutor;
//! use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool};
//! use reth_transaction_pool::blobstore::InMemoryBlobStore;
//! use reth_transaction_pool::maintain::maintain_transaction_pool_future;
//!  async fn t<C, St>(client: C, stream: St)
//!    where C: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
//!     St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
//!     {
//!     let blob_store = InMemoryBlobStore::default();
//!     let pool = Pool::eth_pool(
//!         TransactionValidationTaskExecutor::eth(client.clone(), MAINNET.clone(), blob_store.clone(), TokioTaskExecutor::default()),
//!         blob_store,
//!         Default::default(),
//!     );
//!
//!   // spawn a task that listens for new blocks and updates the pool's transactions, mined transactions etc..
//!   tokio::task::spawn(  maintain_transaction_pool_future(client, pool, stream, TokioTaskExecutor::default(), Default::default()));
//!
//! # }
//! ```
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use crate::pool::PoolInner;
use aquamarine as _;
use reth_primitives::{Address, BlobTransactionSidecar, PooledTransactionsElement, TxHash, U256};
use reth_provider::StateProviderFactory;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::mpsc::Receiver;
use tracing::{instrument, trace};

pub use crate::{
    blobstore::{BlobStore, BlobStoreError},
    config::{
        LocalTransactionConfig, PoolConfig, PriceBumpConfig, SubPoolLimit, DEFAULT_PRICE_BUMP,
        REPLACE_BLOB_PRICE_BUMP, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
        TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT, TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
    },
    error::PoolResult,
    ordering::{CoinbaseTipOrdering, Priority, TransactionOrdering},
    pool::{
        blob_tx_priority, fee_delta, state::SubPool, AllTransactionsEvents, FullTransactionEvent,
        TransactionEvent, TransactionEvents,
    },
    traits::*,
    validate::{
        EthTransactionValidator, TransactionValidationOutcome, TransactionValidationTaskExecutor,
        TransactionValidator, ValidPoolTransaction,
    },
};

pub mod error;
pub mod maintain;
pub mod metrics;
pub mod noop;
pub mod pool;
pub mod validate;

pub mod blobstore;
mod config;
mod identifier;
mod ordering;
mod traits;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking a pool
pub mod test_utils;

/// Type alias for default ethereum transaction pool
pub type EthTransactionPool<Client, S> = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<Client, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    S,
>;

/// A shareable, generic, customizable `TransactionPool` implementation.
#[derive(Debug)]
pub struct Pool<V, T: TransactionOrdering, S> {
    /// Arc'ed instance of the pool internals
    pool: Arc<PoolInner<V, T, S>>,
}

// === impl Pool ===

impl<V, T, S> Pool<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    /// Create a new transaction pool instance.
    pub fn new(validator: V, ordering: T, blob_store: S, config: PoolConfig) -> Self {
        Self { pool: Arc::new(PoolInner::new(validator, ordering, blob_store, config)) }
    }

    /// Returns the wrapped pool.
    pub(crate) fn inner(&self) -> &PoolInner<V, T, S> {
        &self.pool
    }

    /// Get the config the pool was configured with.
    pub fn config(&self) -> &PoolConfig {
        self.inner().config()
    }

    /// Returns future that validates all transaction in the given iterator.
    ///
    /// This returns the validated transactions in the iterator's order.
    async fn validate_all(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = V::Transaction>,
    ) -> PoolResult<Vec<(TxHash, TransactionValidationOutcome<V::Transaction>)>> {
        let outcomes = futures_util::future::join_all(
            transactions.into_iter().map(|tx| self.validate(origin, tx)),
        )
        .await
        .into_iter()
        .collect();

        Ok(outcomes)
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

impl<Client, S> EthTransactionPool<Client, S>
where
    Client: StateProviderFactory + reth_provider::BlockReaderIdExt + Clone + 'static,
    S: BlobStore,
{
    /// Returns a new [Pool] that uses the default [TransactionValidationTaskExecutor] when
    /// validating [EthPooledTransaction]s and ords via [CoinbaseTipOrdering]
    ///
    /// # Example
    ///
    /// ```
    /// use reth_primitives::MAINNET;
    /// use reth_provider::{BlockReaderIdExt, StateProviderFactory};
    /// use reth_tasks::TokioTaskExecutor;
    /// use reth_transaction_pool::{
    ///     blobstore::InMemoryBlobStore, Pool, TransactionValidationTaskExecutor,
    /// };
    /// # fn t<C>(client: C)  where C: StateProviderFactory + BlockReaderIdExt + Clone + 'static {
    /// let blob_store = InMemoryBlobStore::default();
    /// let pool = Pool::eth_pool(
    ///     TransactionValidationTaskExecutor::eth(
    ///         client,
    ///         MAINNET.clone(),
    ///         blob_store.clone(),
    ///         TokioTaskExecutor::default(),
    ///     ),
    ///     blob_store,
    ///     Default::default(),
    /// );
    /// # }
    /// ```
    pub fn eth_pool(
        validator: TransactionValidationTaskExecutor<
            EthTransactionValidator<Client, EthPooledTransaction>,
        >,
        blob_store: S,
        config: PoolConfig,
    ) -> Self {
        Self::new(validator, CoinbaseTipOrdering::default(), blob_store, config)
    }
}

/// implements the `TransactionPool` interface for various transaction pool API consumers.
#[async_trait::async_trait]
impl<V, T, S> TransactionPool for Pool<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    type Transaction = T::Transaction;

    fn pool_size(&self) -> PoolSize {
        self.pool.size()
    }

    fn block_info(&self) -> BlockInfo {
        self.pool.block_info()
    }

    async fn add_transaction_and_subscribe(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TransactionEvents> {
        let (_, tx) = self.validate(origin, transaction).await;
        self.pool.add_transaction_and_subscribe(origin, tx)
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
        if transactions.is_empty() {
            return Ok(Vec::new())
        }
        let validated = self.validate_all(origin, transactions).await?;

        let transactions =
            self.pool.add_transactions(origin, validated.into_iter().map(|(_, tx)| tx));
        Ok(transactions)
    }

    fn transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents> {
        self.pool.add_transaction_event_listener(tx_hash)
    }

    fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction> {
        self.pool.add_all_transactions_event_listener()
    }

    fn pending_transactions_listener_for(&self, kind: TransactionListenerKind) -> Receiver<TxHash> {
        self.pool.add_pending_listener(kind)
    }

    fn blob_transaction_sidecars_listener(&self) -> Receiver<NewBlobSidecar> {
        self.pool.add_blob_sidecar_listener()
    }

    fn new_transactions_listener_for(
        &self,
        kind: TransactionListenerKind,
    ) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        self.pool.add_new_transaction_listener(kind)
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

    fn get_pooled_transaction_elements(
        &self,
        tx_hashes: Vec<TxHash>,
        limit: GetPooledTransactionLimit,
    ) -> Vec<PooledTransactionsElement> {
        self.pool.get_pooled_transaction_elements(tx_hashes, limit)
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.best_transactions())
    }

    fn best_transactions_with_base_fee(
        &self,
        base_fee: u64,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        self.pool.best_transactions_with_base_fee(base_fee)
    }

    fn best_transactions_with_attributes(
        &self,
        best_transactions_attributes: BestTransactionsAttributes,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        self.pool.best_transactions_with_attributes(best_transactions_attributes)
    }

    fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pending_transactions()
    }

    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.queued_transactions()
    }

    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction> {
        self.pool.all_transactions()
    }

    fn remove_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_transactions(hashes)
    }

    fn retain_unknown(&self, hashes: &mut Vec<TxHash>) {
        self.pool.retain_unknown(hashes)
    }

    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner().get(tx_hash)
    }

    fn get_all(&self, txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
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

    fn get_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_transactions_by_origin(origin)
    }

    fn unique_senders(&self) -> HashSet<Address> {
        self.pool.unique_senders()
    }

    fn get_blob(&self, tx_hash: TxHash) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        self.pool.blob_store().get(tx_hash)
    }

    fn get_all_blobs(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<(TxHash, BlobTransactionSidecar)>, BlobStoreError> {
        self.pool.blob_store().get_all(tx_hashes)
    }

    fn get_all_blobs_exact(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        self.pool.blob_store().get_exact(tx_hashes)
    }
}

impl<V: TransactionValidator, T: TransactionOrdering, S> TransactionPoolExt for Pool<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    #[instrument(skip(self), target = "txpool")]
    fn set_block_info(&self, info: BlockInfo) {
        trace!(target: "txpool", "updating pool block info");
        self.pool.set_block_info(info)
    }

    fn on_canonical_state_change(&self, update: CanonicalStateUpdate<'_>) {
        self.pool.on_canonical_state_change(update);
    }

    fn update_accounts(&self, accounts: Vec<ChangedAccount>) {
        self.pool.update_accounts(accounts);
    }

    fn delete_blob(&self, tx: TxHash) {
        self.pool.delete_blob(tx)
    }

    fn delete_blobs(&self, txs: Vec<TxHash>) {
        self.pool.delete_blobs(txs)
    }
}

impl<V, T: TransactionOrdering, S> Clone for Pool<V, T, S> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}
