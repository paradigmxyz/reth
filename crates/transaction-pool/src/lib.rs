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
//! ## Transaction Flow: From Network/RPC to Pool
//!
//! Transactions enter the pool through two main paths:
//!
//! ### 1. Network Path (P2P)
//!
//! ```text
//! Network Peer
//!     ↓
//! Transactions or NewPooledTransactionHashes message
//!     ↓
//! TransactionsManager (crates/net/network/src/transactions/mod.rs)
//!     │
//!     ├─→ For Transactions message:
//!     │   ├─→ Validates message format
//!     │   ├─→ Checks if transaction already known
//!     │   ├─→ Marks peer as having seen the transaction
//!     │   └─→ Queues for import
//!     │
//!     └─→ For NewPooledTransactionHashes message:
//!         ├─→ Filters out already known transactions
//!         ├─→ Queues unknown hashes for fetching
//!         ├─→ Sends GetPooledTransactions request
//!         ├─→ Receives PooledTransactions response
//!         └─→ Queues fetched transactions for import
//!             ↓
//! pool.add_external_transactions() [Origin: External]
//!     ↓
//! Transaction Validation & Pool Addition
//! ```
//!
//! ### 2. RPC Path (Local submission)
//!
//! ```text
//! eth_sendRawTransaction RPC call
//!     ├─→ Decodes raw bytes
//!     └─→ Recovers sender
//!         ↓
//! pool.add_transaction() [Origin: Local]
//!     ↓
//! Transaction Validation & Pool Addition
//! ```
//!
//! ### Transaction Origins
//!
//! - **Local**: Transactions submitted via RPC (trusted, may have different fee requirements)
//! - **External**: Transactions from network peers (untrusted, subject to stricter validation)
//! - **Private**: Local transactions that should not be propagated to the network
//!
//! ## Validation Process
//!
//! ### Stateless Checks
//!
//! Ethereum transactions undergo several stateless checks:
//!
//! - **Transaction Type**: Fork-dependent support (Legacy always, EIP-2930/1559/4844/7702 need
//!   activation)
//! - **Size**: Input data ≤ 128KB (default)
//! - **Gas**: Limit ≤ block gas limit
//! - **Fees**: Priority fee ≤ max fee; local tx fee cap; external minimum priority fee
//! - **Chain ID**: Must match current chain
//! - **Intrinsic Gas**: Sufficient for data and access lists
//! - **Blobs** (EIP-4844): Valid count, KZG proofs
//!
//! ### Stateful Checks
//!
//! 1. **Sender**: No bytecode (unless EIP-7702 delegated in Prague)
//! 2. **Nonce**: ≥ account nonce
//! 3. **Balance**: Covers value + (`gas_limit` × `max_fee_per_gas`)
//!
//! ### Common Errors
//!
//! - [`NonceNotConsistent`](reth_primitives_traits::transaction::error::InvalidTransactionError::NonceNotConsistent): Nonce too low
//! - [`InsufficientFunds`](reth_primitives_traits::transaction::error::InvalidTransactionError::InsufficientFunds): Insufficient balance
//! - [`ExceedsGasLimit`](crate::error::InvalidPoolTransactionError::ExceedsGasLimit): Gas limit too
//!   high
//! - [`SignerAccountHasBytecode`](reth_primitives_traits::transaction::error::InvalidTransactionError::SignerAccountHasBytecode): EOA has code
//! - [`Underpriced`](crate::error::InvalidPoolTransactionError::Underpriced): Fee too low
//! - [`ReplacementUnderpriced`](crate::error::PoolErrorKind::ReplacementUnderpriced): Replacement
//!   transaction fee too low
//! - Blob errors:
//!   - [`MissingEip4844BlobSidecar`](crate::error::Eip4844PoolTransactionError::MissingEip4844BlobSidecar): Missing sidecar
//!   - [`InvalidEip4844Blob`](crate::error::Eip4844PoolTransactionError::InvalidEip4844Blob):
//!     Invalid blob proofs
//!   - [`NoEip4844Blobs`](crate::error::Eip4844PoolTransactionError::NoEip4844Blobs): EIP-4844
//!     transaction without blobs
//!   - [`TooManyEip4844Blobs`](crate::error::Eip4844PoolTransactionError::TooManyEip4844Blobs): Too
//!     many blobs
//!
//! ## Subpool Design
//!
//! The pool maintains four distinct subpools, each serving a specific purpose
//!
//! ### Subpools
//!
//! 1. **Pending**: Ready for inclusion (no gaps, sufficient balance/fees)
//! 2. **Queued**: Future transactions (nonce gaps or insufficient balance)
//! 3. **`BaseFee`**: Valid but below current base fee
//! 4. **Blob**: EIP-4844 transactions not pending due to insufficient base fee or blob fee
//!
//! ### State Transitions
//!
//! Transactions move between subpools based on state changes:
//!
//! ```text
//! Queued ─────────→ BaseFee/Blob ────────→ Pending
//!   ↑                      ↑                       │
//!   │                      │                       │
//!   └────────────────────┴─────────────────────┘
//!         (demotions due to state changes)
//! ```
//!
//! **Promotions**: Nonce gaps filled, balance/fee improvements
//! **Demotions**: Nonce gaps created, balance/fee degradation
//!
//! ## Pool Maintenance
//!
//! 1. **Block Updates**: Removes mined txs, updates accounts/fees, triggers movements
//! 2. **Size Enforcement**: Discards worst transactions when limits exceeded
//! 3. **Propagation**: External (always), Local (configurable), Private (never)
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
//! the current state or could become valid after certain state changes. Transactions that can never
//! become valid (e.g. nonce lower than current on chain nonce) will never be added to the pool and
//! instead are discarded right away.
//!
//! ### State Changes
//!
//! New blocks trigger pool updates via changesets (see Pool Maintenance).
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
//! See also [`ValidTransaction`](validate::ValidTransaction)
//!
//!
//! ## Examples
//!
//! Listen for new transactions and print them:
//!
//! ```
//! use reth_chainspec::MAINNET;
//! use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
//! use reth_tasks::TokioTaskExecutor;
//! use reth_chainspec::ChainSpecProvider;
//! use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool, TransactionPool};
//! use reth_transaction_pool::blobstore::InMemoryBlobStore;
//! use reth_chainspec::EthereumHardforks;
//! use reth_evm::ConfigureEvm;
//! use alloy_consensus::Header;
//! async fn t<C, Evm>(client: C, evm_config: Evm)
//! where
//!     C: ChainSpecProvider<ChainSpec: EthereumHardforks> + StateProviderFactory + BlockReaderIdExt<Header = Header> + Clone + 'static,
//!     Evm: ConfigureEvm<Primitives: reth_primitives_traits::NodePrimitives<BlockHeader = Header>> + 'static,
//! {
//!     let blob_store = InMemoryBlobStore::default();
//!     let pool = Pool::eth_pool(
//!         TransactionValidationTaskExecutor::eth(client, evm_config, blob_store.clone(), TokioTaskExecutor::default()),
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
//! use reth_chain_state::CanonStateNotification;
//! use reth_chainspec::{MAINNET, ChainSpecProvider, ChainSpec};
//! use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
//! use reth_tasks::TokioTaskExecutor;
//! use reth_tasks::TaskSpawner;
//! use reth_tasks::Runtime;
//! use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool};
//! use reth_transaction_pool::blobstore::InMemoryBlobStore;
//! use reth_transaction_pool::maintain::{maintain_transaction_pool_future};
//! use reth_evm::ConfigureEvm;
//! use reth_ethereum_primitives::EthPrimitives;
//! use alloy_consensus::Header;
//!
//!  async fn t<C, St, Evm>(client: C, stream: St, evm_config: Evm)
//!    where C: StateProviderFactory + BlockReaderIdExt<Header = Header> + ChainSpecProvider<ChainSpec = ChainSpec> + Clone + 'static,
//!     St: Stream<Item = CanonStateNotification<EthPrimitives>> + Send + Unpin + 'static,
//!     Evm: ConfigureEvm<Primitives = EthPrimitives> + 'static,
//!     {
//!     let blob_store = InMemoryBlobStore::default();
//!     let rt = tokio::runtime::Runtime::new().unwrap();
//!     let runtime = Runtime::with_existing_handle(rt.handle().clone()).unwrap();
//!     let pool = Pool::eth_pool(
//!         TransactionValidationTaskExecutor::eth(client.clone(), evm_config, blob_store.clone(), runtime.clone()),
//!         blob_store,
//!         Default::default(),
//!     );
//!
//!   // spawn a task that listens for new blocks and updates the pool's transactions, mined transactions etc..
//!   tokio::task::spawn(maintain_transaction_pool_future(client, pool, stream, runtime.clone(), Default::default()));
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
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

pub use crate::{
    batcher::{BatchTxProcessor, BatchTxRequest},
    blobstore::{BlobStore, BlobStoreError},
    config::{
        LocalTransactionConfig, PoolConfig, PriceBumpConfig, SubPoolLimit,
        DEFAULT_MAX_INFLIGHT_DELEGATED_SLOTS, DEFAULT_PRICE_BUMP,
        DEFAULT_TXPOOL_ADDITIONAL_VALIDATION_TASKS, MAX_NEW_PENDING_TXS_NOTIFICATIONS,
        REPLACE_BLOB_PRICE_BUMP, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
        TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT, TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
    },
    error::PoolResult,
    ordering::{CoinbaseTipOrdering, Priority, TransactionOrdering},
    pool::{
        blob_tx_priority, fee_delta, state::SubPool, AddedTransactionOutcome,
        AllTransactionsEvents, FullTransactionEvent, NewTransactionEvent, TransactionEvent,
        TransactionEvents, TransactionListenerKind,
    },
    traits::*,
    validate::{
        EthTransactionValidator, TransactionValidationOutcome, TransactionValidationTaskExecutor,
        TransactionValidator, ValidPoolTransaction,
    },
};
use crate::{identifier::TransactionId, pool::PoolInner};
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2},
    eip7594::BlobTransactionSidecarVariant,
};
use alloy_primitives::{map::AddressSet, Address, TxHash, B256, U256};
use aquamarine as _;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_eth_wire_types::HandleMempoolData;
use reth_evm::ConfigureEvm;
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::ChangedAccount;
use reth_primitives_traits::{HeaderTy, Recovered};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tracing::{instrument, trace};

pub mod error;
pub mod maintain;
pub mod metrics;
pub mod noop;
pub mod orderflow;
pub mod pool;
pub mod validate;

pub mod batcher;
pub mod blobstore;
mod config;
pub mod identifier;
mod ordering;
mod traits;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking a pool
pub mod test_utils;

/// Type alias for default ethereum transaction pool
pub type EthTransactionPool<Client, S, Evm = EthEvmConfig, T = EthPooledTransaction> = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<Client, T, Evm>>,
    CoinbaseTipOrdering<T>,
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

    /// Returns the wrapped pool internals.
    pub fn inner(&self) -> &PoolInner<V, T, S> {
        &self.pool
    }

    /// Get the config the pool was configured with.
    pub fn config(&self) -> &PoolConfig {
        self.inner().config()
    }

    /// Get the validator reference.
    pub fn validator(&self) -> &V {
        self.inner().validator()
    }

    /// Validates the given transaction
    async fn validate(
        &self,
        origin: TransactionOrigin,
        transaction: V::Transaction,
    ) -> TransactionValidationOutcome<V::Transaction> {
        self.pool.validator().validate_transaction(origin, transaction).await
    }

    /// Number of transactions in the entire pool
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// Whether the pool is empty
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    /// Returns whether or not the pool is over its configured size and transaction count limits.
    pub fn is_exceeded(&self) -> bool {
        self.pool.is_exceeded()
    }

    /// Returns the configured blob store.
    pub fn blob_store(&self) -> &S {
        self.pool.blob_store()
    }
}

impl<Client, S, Evm> EthTransactionPool<Client, S, Evm>
where
    Client: ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + Clone
        + BlockReaderIdExt<Header = HeaderTy<Evm::Primitives>>
        + 'static,
    S: BlobStore,
    Evm: ConfigureEvm + 'static,
{
    /// Returns a new [`Pool`] that uses the default [`TransactionValidationTaskExecutor`] when
    /// validating [`EthPooledTransaction`]s and ords via [`CoinbaseTipOrdering`]
    ///
    /// # Example
    ///
    /// ```
    /// use reth_chainspec::MAINNET;
    /// use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
    /// use reth_tasks::TokioTaskExecutor;
    /// use reth_chainspec::ChainSpecProvider;
    /// use reth_transaction_pool::{
    ///     blobstore::InMemoryBlobStore, Pool, TransactionValidationTaskExecutor,
    /// };
    /// use reth_chainspec::EthereumHardforks;
    /// use reth_evm::ConfigureEvm;
    /// use alloy_consensus::Header;
    /// # fn t<C, Evm>(client: C, evm_config: Evm)
    /// # where
    /// #     C: ChainSpecProvider<ChainSpec: EthereumHardforks> + StateProviderFactory + BlockReaderIdExt<Header = Header> + Clone + 'static,
    /// #     Evm: ConfigureEvm<Primitives: reth_primitives_traits::NodePrimitives<BlockHeader = Header>> + 'static,
    /// # {
    /// let blob_store = InMemoryBlobStore::default();
    /// let pool = Pool::eth_pool(
    ///     TransactionValidationTaskExecutor::eth(
    ///         client,
    ///         evm_config,
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
            EthTransactionValidator<Client, EthPooledTransaction, Evm>,
        >,
        blob_store: S,
        config: PoolConfig,
    ) -> Self {
        Self::new(validator, CoinbaseTipOrdering::default(), blob_store, config)
    }
}

/// implements the `TransactionPool` interface for various transaction pool API consumers.
impl<V, T, S> TransactionPool for Pool<V, T, S>
where
    V: TransactionValidator,
    <V as TransactionValidator>::Transaction: EthPoolTransaction,
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
        let tx = self.validate(origin, transaction).await;
        self.pool.add_transaction_and_subscribe(origin, tx)
    }

    async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<AddedTransactionOutcome> {
        let tx = self.validate(origin, transaction).await;
        let mut results = self.pool.add_transactions(origin, std::iter::once(tx));
        results.pop().expect("result length is the same as the input")
    }

    async fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        if transactions.is_empty() {
            return Vec::new()
        }
        let validated = self
            .pool
            .validator()
            .validate_transactions(transactions.into_iter().map(|tx| (origin, tx)))
            .await;
        self.pool.add_transactions(origin, validated)
    }

    async fn add_transactions_with_origins(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        if transactions.is_empty() {
            return Vec::new()
        }
        let origins: Vec<_> = transactions.iter().map(|(origin, _)| *origin).collect();
        let validated = self.pool.validator().validate_transactions(transactions).await;
        self.pool.add_transactions_with_origins(origins.into_iter().zip(validated))
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
        self.pool.pooled_transactions_hashes_max(max)
    }

    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pooled_transactions()
    }

    fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pooled_transactions_max(max)
    }

    fn get_pooled_transaction_elements(
        &self,
        tx_hashes: Vec<TxHash>,
        limit: GetPooledTransactionLimit,
    ) -> Vec<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled> {
        self.pool.get_pooled_transaction_elements(tx_hashes, limit)
    }

    fn append_pooled_transaction_elements(
        &self,
        tx_hashes: &[TxHash],
        limit: GetPooledTransactionLimit,
        out: &mut Vec<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled>,
    ) {
        self.pool.append_pooled_transaction_elements(tx_hashes, limit, out)
    }

    fn get_pooled_transaction_element(
        &self,
        tx_hash: TxHash,
    ) -> Option<Recovered<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled>>
    {
        self.pool.get_pooled_transaction_element(tx_hash)
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.best_transactions())
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

    fn get_pending_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_pending_transaction_by_sender_and_nonce(sender, nonce)
    }

    fn pending_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pending_transactions_max(max)
    }

    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.queued_transactions()
    }

    fn pending_and_queued_txn_count(&self) -> (usize, usize) {
        let data = self.pool.get_pool_data();
        let pending = data.pending_transactions_count();
        let queued = data.queued_transactions_count();
        (pending, queued)
    }

    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction> {
        self.pool.all_transactions()
    }

    fn all_transaction_hashes(&self) -> Vec<TxHash> {
        self.pool.all_transaction_hashes()
    }

    fn remove_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_transactions(hashes)
    }

    fn remove_transactions_and_descendants(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_transactions_and_descendants(hashes)
    }

    fn remove_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_transactions_by_sender(sender)
    }

    fn prune_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.prune_transactions(hashes)
    }

    fn retain_unknown<A>(&self, announcement: &mut A)
    where
        A: HandleMempoolData,
    {
        self.pool.retain_unknown(announcement)
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

    fn get_pending_transactions_with_predicate(
        &self,
        predicate: impl FnMut(&ValidPoolTransaction<Self::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pending_transactions_with_predicate(predicate)
    }

    fn get_pending_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_pending_transactions_by_sender(sender)
    }

    fn get_queued_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_queued_transactions_by_sender(sender)
    }

    fn get_highest_transaction_by_sender(
        &self,
        sender: Address,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_highest_transaction_by_sender(sender)
    }

    fn get_highest_consecutive_transaction_by_sender(
        &self,
        sender: Address,
        on_chain_nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_highest_consecutive_transaction_by_sender(sender, on_chain_nonce)
    }

    fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let transaction_id = TransactionId::new(self.pool.get_sender_id(sender), nonce);

        self.inner().get_pool_data().all().get(&transaction_id).map(|tx| tx.transaction.clone())
    }

    fn get_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_transactions_by_origin(origin)
    }

    /// Returns all pending transactions filtered by [`TransactionOrigin`]
    fn get_pending_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_pending_transactions_by_origin(origin)
    }

    fn unique_senders(&self) -> AddressSet {
        self.pool.unique_senders()
    }

    fn get_blob(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        self.pool.blob_store().get(tx_hash)
    }

    fn get_all_blobs(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<(TxHash, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
        self.pool.blob_store().get_all(tx_hashes)
    }

    fn get_all_blobs_exact(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        self.pool.blob_store().get_exact(tx_hashes)
    }

    fn get_blobs_for_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        self.pool.blob_store().get_by_versioned_hashes_v1(versioned_hashes)
    }

    fn get_blobs_for_versioned_hashes_v2(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError> {
        self.pool.blob_store().get_by_versioned_hashes_v2(versioned_hashes)
    }

    fn get_blobs_for_versioned_hashes_v3(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError> {
        self.pool.blob_store().get_by_versioned_hashes_v3(versioned_hashes)
    }
}

impl<V, T, S> TransactionPoolExt for Pool<V, T, S>
where
    V: TransactionValidator,
    <V as TransactionValidator>::Transaction: EthPoolTransaction,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    type Block = V::Block;

    #[instrument(skip(self), target = "txpool")]
    fn set_block_info(&self, info: BlockInfo) {
        trace!(target: "txpool", "updating pool block info");
        self.pool.set_block_info(info)
    }

    fn on_canonical_state_change(&self, update: CanonicalStateUpdate<'_, Self::Block>) {
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

    fn cleanup_blobs(&self) {
        self.pool.cleanup_blobs()
    }
}

impl<V, T: TransactionOrdering, S> Clone for Pool<V, T, S> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}
