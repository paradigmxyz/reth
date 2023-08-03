use crate::{
    error::PoolResult,
    pool::{state::SubPool, TransactionEvents},
    validate::ValidPoolTransaction,
    AllTransactionsEvents,
};
use futures_util::{ready, Stream};
use reth_primitives::{
    Address, FromRecoveredTransaction, IntoRecoveredTransaction, PeerId, Transaction,
    TransactionKind, TransactionSignedEcRecovered, TxHash, EIP1559_TX_TYPE_ID, H256, U256,
};
use reth_rlp::Encodable;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc::Receiver;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// General purpose abstraction fo a transaction-pool.
///
/// This is intended to be used by API-consumers such as RPC that need inject new incoming,
/// unverified transactions. And by block production that needs to get transactions to execute in a
/// new block.
///
/// Note: This requires `Clone` for convenience, since it is assumed that this will be implemented
/// for a wrapped `Arc` type, see also [`Pool`](crate::Pool).
#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait TransactionPool: Send + Sync + Clone {
    /// The transaction type of the pool
    type Transaction: PoolTransaction;

    /// Returns stats about the pool and all sub-pools.
    fn pool_size(&self) -> PoolSize;

    /// Returns the block the pool is currently tracking.
    ///
    /// This tracks the block that the pool has last seen.
    fn block_info(&self) -> BlockInfo;

    /// Imports an _external_ transaction.
    ///
    /// This is intended to be used by the network to insert incoming transactions received over the
    /// p2p network.
    ///
    /// Consumer: P2P
    async fn add_external_transaction(&self, transaction: Self::Transaction) -> PoolResult<TxHash> {
        self.add_transaction(TransactionOrigin::External, transaction).await
    }

    /// Imports all _external_ transactions
    ///
    ///
    /// Consumer: Utility
    async fn add_external_transactions(
        &self,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        self.add_transactions(TransactionOrigin::External, transactions).await
    }

    /// Adds an _unvalidated_ transaction into the pool and subscribe to state changes.
    ///
    /// This is the same as [TransactionPool::add_transaction] but returns an event stream for the
    /// given transaction.
    ///
    /// Consumer: Custom
    async fn add_transaction_and_subscribe(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TransactionEvents>;

    /// Adds an _unvalidated_ transaction into the pool.
    ///
    /// Consumer: RPC
    async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash>;

    /// Adds the given _unvalidated_ transaction into the pool.
    ///
    /// Returns a list of results.
    ///
    /// Consumer: RPC
    async fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>>;

    /// Returns a new transaction change event stream for the given transaction.
    ///
    /// Returns `None` if the transaction is not in the pool.
    fn transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents>;

    /// Returns a new transaction change event stream for _all_ transactions in the pool.
    fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction>;

    /// Returns a new Stream that yields transactions hashes for new __pending__ transactions
    /// inserted into the pool that are allowed to be propagated.
    ///
    /// Note: This is intended for networking and will __only__ yield transactions that are allowed
    /// to be propagated over the network.
    ///
    /// Consumer: RPC/P2P
    fn pending_transactions_listener(&self) -> Receiver<TxHash> {
        self.pending_transactions_listener_for(PendingTransactionListenerKind::PropagateOnly)
    }

    /// Returns a new Stream that yields transactions hashes for new __pending__ transactions
    /// inserted into the pool depending on the given [PendingTransactionListenerKind] argument.
    fn pending_transactions_listener_for(
        &self,
        kind: PendingTransactionListenerKind,
    ) -> Receiver<TxHash>;

    /// Returns a new stream that yields new valid transactions added to the pool.
    fn new_transactions_listener(&self) -> Receiver<NewTransactionEvent<Self::Transaction>>;

    /// Returns a new Stream that yields new transactions added to the basefee-pool.
    ///
    /// This is a convenience wrapper around [Self::new_transactions_listener] that filters for
    /// [SubPool::Pending](crate::SubPool).
    fn new_pending_pool_transactions_listener(
        &self,
    ) -> NewSubpoolTransactionStream<Self::Transaction> {
        NewSubpoolTransactionStream::new(self.new_transactions_listener(), SubPool::Pending)
    }

    /// Returns a new Stream that yields new transactions added to the basefee sub-pool.
    ///
    /// This is a convenience wrapper around [Self::new_transactions_listener] that filters for
    /// [SubPool::BaseFee](crate::SubPool).
    fn new_basefee_pool_transactions_listener(
        &self,
    ) -> NewSubpoolTransactionStream<Self::Transaction> {
        NewSubpoolTransactionStream::new(self.new_transactions_listener(), SubPool::BaseFee)
    }

    /// Returns a new Stream that yields new transactions added to the queued-pool.
    ///
    /// This is a convenience wrapper around [Self::new_transactions_listener] that filters for
    /// [SubPool::Queued](crate::SubPool).
    fn new_queued_transactions_listener(&self) -> NewSubpoolTransactionStream<Self::Transaction> {
        NewSubpoolTransactionStream::new(self.new_transactions_listener(), SubPool::Queued)
    }

    /// Returns the _hashes_ of all transactions in the pool.
    ///
    /// Note: This returns a `Vec` but should guarantee that all hashes are unique.
    ///
    /// Consumer: P2P
    fn pooled_transaction_hashes(&self) -> Vec<TxHash>;

    /// Returns only the first `max` hashes of transactions in the pool.
    ///
    /// Consumer: P2P
    fn pooled_transaction_hashes_max(&self, max: usize) -> Vec<TxHash>;

    /// Returns the _full_ transaction objects all transactions in the pool.
    ///
    /// Note: This returns a `Vec` but should guarantee that all transactions are unique.
    ///
    /// Consumer: P2P
    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns only the first `max` transactions in the pool.
    ///
    /// Consumer: P2P
    fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns an iterator that yields transactions that are ready for block production.
    ///
    /// Consumer: Block production
    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;

    /// Returns an iterator that yields transactions that are ready for block production with the
    /// given base fee.
    ///
    /// Consumer: Block production
    fn best_transactions_with_base_fee(
        &self,
        base_fee: u64,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;

    /// Returns all transactions that can be included in the next block.
    ///
    /// This is primarily used for the `txpool_` RPC namespace: <https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-txpool> which distinguishes between `pending` and `queued` transactions, where `pending` are transactions ready for inclusion in the next block and `queued` are transactions that are ready for inclusion in future blocks.
    ///
    /// Consumer: RPC
    fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns all transactions that can be included in _future_ blocks.
    ///
    /// This and [Self::pending_transactions] are mutually exclusive.
    ///
    /// Consumer: RPC
    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns all transactions that are currently in the pool grouped by whether they are ready
    /// for inclusion in the next block or not.
    ///
    /// This is primarily used for the `txpool_` namespace: <https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-txpool>
    ///
    /// Consumer: RPC
    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction>;

    /// Removes all transactions corresponding to the given hashes.
    ///
    /// Also removes all _dependent_ transactions.
    ///
    /// Consumer: Block production
    fn remove_transactions(
        &self,
        hashes: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Retains only those hashes that are unknown to the pool.
    /// In other words, removes all transactions from the given set that are currently present in
    /// the pool.
    ///
    /// Consumer: P2P
    fn retain_unknown(&self, hashes: &mut Vec<TxHash>);

    /// Returns if the transaction for the given hash is already included in this pool.
    fn contains(&self, tx_hash: &TxHash) -> bool {
        self.get(tx_hash).is_some()
    }

    /// Returns the transaction for the given hash.
    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns all transactions objects for the given hashes.
    ///
    /// This adheres to the expected behavior of [`GetPooledTransactions`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09):
    /// The transactions must be in same order as in the request, but it is OK to skip transactions
    /// which are not available.
    fn get_all(
        &self,
        txs: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Notify the pool about transactions that are propagated to peers.
    ///
    /// Consumer: P2P
    fn on_propagated(&self, txs: PropagatedTransactions);

    /// Returns all transactions sent by a given user
    fn get_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;

    /// Returns a set of all senders of transactions in the pool
    fn unique_senders(&self) -> HashSet<Address>;
}

/// Extension for [TransactionPool] trait that allows to set the current block info.
#[auto_impl::auto_impl(Arc)]
pub trait TransactionPoolExt: TransactionPool {
    /// Sets the current block info for the pool.
    fn set_block_info(&self, info: BlockInfo);

    /// Event listener for when the pool needs to be updated
    ///
    /// Implementers need to update the pool accordingly.
    /// For example the base fee of the pending block is determined after a block is mined which
    /// affects the dynamic fee requirement of pending transactions in the pool.
    fn on_canonical_state_change(&self, update: CanonicalStateUpdate);

    /// Updates the accounts in the pool
    fn update_accounts(&self, accounts: Vec<ChangedAccount>);
}

/// Determines what kind of new pending transactions should be emitted by a stream of pending
/// transactions.
///
/// This gives control whether to include transactions that are allowed to be propagated.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PendingTransactionListenerKind {
    /// Any new pending transactions
    All,
    /// Only transactions that are allowed to be propagated.
    ///
    /// See also [ValidPoolTransaction]
    PropagateOnly,
}

impl PendingTransactionListenerKind {
    /// Returns true if we're only interested in transactions that are allowed to be propagated.
    #[inline]
    pub fn is_propagate_only(&self) -> bool {
        matches!(self, Self::PropagateOnly)
    }
}

/// A Helper type that bundles all transactions in the pool.
#[derive(Debug, Clone)]
pub struct AllPoolTransactions<T: PoolTransaction> {
    /// Transactions that are ready for inclusion in the next block.
    pub pending: Vec<Arc<ValidPoolTransaction<T>>>,
    /// Transactions that are ready for inclusion in _future_ blocks, but are currently parked,
    /// because they depend on other transactions that are not yet included in the pool (nonce gap)
    /// or otherwise blocked.
    pub queued: Vec<Arc<ValidPoolTransaction<T>>>,
}

// === impl AllPoolTransactions ===

impl<T: PoolTransaction> AllPoolTransactions<T> {
    /// Returns an iterator over all pending [TransactionSignedEcRecovered] transactions.
    pub fn pending_recovered(&self) -> impl Iterator<Item = TransactionSignedEcRecovered> + '_ {
        self.pending.iter().map(|tx| tx.transaction.to_recovered_transaction())
    }

    /// Returns an iterator over all queued [TransactionSignedEcRecovered] transactions.
    pub fn queued_recovered(&self) -> impl Iterator<Item = TransactionSignedEcRecovered> + '_ {
        self.queued.iter().map(|tx| tx.transaction.to_recovered_transaction())
    }
}

impl<T: PoolTransaction> Default for AllPoolTransactions<T> {
    fn default() -> Self {
        Self { pending: Default::default(), queued: Default::default() }
    }
}

/// Represents a transaction that was propagated over the network.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct PropagatedTransactions(pub HashMap<TxHash, Vec<PropagateKind>>);

/// Represents how a transaction was propagated over the network.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum PropagateKind {
    /// The full transaction object was sent to the peer.
    ///
    /// This is equivalent to the `Transaction` message
    Full(PeerId),
    /// Only the Hash was propagated to the peer.
    Hash(PeerId),
}

// === impl PropagateKind ===

impl PropagateKind {
    /// Returns the peer the transaction was sent to
    pub fn peer(&self) -> &PeerId {
        match self {
            PropagateKind::Full(peer) => peer,
            PropagateKind::Hash(peer) => peer,
        }
    }
}

impl From<PropagateKind> for PeerId {
    fn from(value: PropagateKind) -> Self {
        match value {
            PropagateKind::Full(peer) => peer,
            PropagateKind::Hash(peer) => peer,
        }
    }
}

/// Represents a new transaction
#[derive(Debug)]
pub struct NewTransactionEvent<T: PoolTransaction> {
    /// The pool which the transaction was moved to.
    pub subpool: SubPool,
    /// Actual transaction
    pub transaction: Arc<ValidPoolTransaction<T>>,
}

impl<T: PoolTransaction> Clone for NewTransactionEvent<T> {
    fn clone(&self) -> Self {
        Self { subpool: self.subpool, transaction: self.transaction.clone() }
    }
}

/// Where the transaction originates from.
///
/// Depending on where the transaction was picked up, it affects how the transaction is handled
/// internally, e.g. limits for simultaneous transaction of one sender.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TransactionOrigin {
    /// Transaction is coming from a local source.
    Local,
    /// Transaction has been received externally.
    ///
    /// This is usually considered an "untrusted" source, for example received from another in the
    /// network.
    External,
}

// === impl TransactionOrigin ===

impl TransactionOrigin {
    /// Whether the transaction originates from a local source.
    pub fn is_local(&self) -> bool {
        matches!(self, TransactionOrigin::Local)
    }
}

/// Represents changes after a new canonical block or range of canonical blocks was added to the
/// chain.
///
/// It is expected that this is only used if the added blocks are canonical to the pool's last known
/// block hash. In other words, the first added block of the range must be the child of the last
/// known block hash.
///
/// This is used to update the pool state accordingly.
#[derive(Debug, Clone)]
pub struct CanonicalStateUpdate {
    /// Hash of the tip block.
    pub hash: H256,
    /// Number of the tip block.
    pub number: u64,
    /// EIP-1559 Base fee of the _next_ (pending) block
    ///
    /// The base fee of a block depends on the utilization of the last block and its base fee.
    pub pending_block_base_fee: u64,
    /// A set of changed accounts across a range of blocks.
    pub changed_accounts: Vec<ChangedAccount>,
    /// All mined transactions in the block range.
    pub mined_transactions: Vec<H256>,
    /// Timestamp of the latest chain update
    pub timestamp: u64,
}

impl fmt::Display for CanonicalStateUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{ hash: {}, number: {}, pending_block_base_fee: {}, changed_accounts: {}, mined_transactions: {} }}",
            self.hash, self.number, self.pending_block_base_fee, self.changed_accounts.len(), self.mined_transactions.len())
    }
}

/// Represents a changed account
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChangedAccount {
    /// The address of the account.
    pub address: Address,
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
}

// === impl ChangedAccount ===

impl ChangedAccount {
    /// Creates a new `ChangedAccount` with the given address and 0 balance and nonce.
    pub(crate) fn empty(address: Address) -> Self {
        Self { address, nonce: 0, balance: U256::ZERO }
    }
}

/// An `Iterator` that only returns transactions that are ready to be executed.
///
/// This makes no assumptions about the order of the transactions, but expects that _all_
/// transactions are valid (no nonce gaps.) for the tracked state of the pool.
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
pub trait PoolTransaction:
    fmt::Debug + Send + Sync + FromRecoveredTransaction + IntoRecoveredTransaction
{
    /// Hash of the transaction.
    fn hash(&self) -> &TxHash;

    /// The Sender of the transaction.
    fn sender(&self) -> Address;

    /// Returns the nonce for this transaction.
    fn nonce(&self) -> u64;

    /// Returns the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions: `max_fee_per_gas * gas_limit + tx_value`.
    /// For legacy transactions: `gas_price * gas_limit + tx_value`.
    fn cost(&self) -> U256;

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
    fn gas_limit(&self) -> u64;

    /// Returns the EIP-1559 the maximum fee per gas the caller is willing to pay.
    ///
    /// For legacy transactions this is gas_price.
    ///
    /// This is also commonly referred to as the "Gas Fee Cap" (`GasFeeCap`).
    fn max_fee_per_gas(&self) -> u128;

    /// Returns the EIP-1559 Priority fee the caller is paying to the block author.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_priority_fee_per_gas(&self) -> Option<u128>;

    /// Returns the effective tip for this transaction.
    ///
    /// For EIP-1559 transactions: `min(max_fee_per_gas - base_fee, max_priority_fee_per_gas)`.
    /// For legacy transactions: `gas_price - base_fee`.
    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128>;

    /// Returns the max priority fee per gas if the transaction is an EIP-1559 transaction, and
    /// otherwise returns the gas price.
    fn priority_fee_or_price(&self) -> u128;

    /// Returns the transaction's [`TransactionKind`], which is the address of the recipient or
    /// [`TransactionKind::Create`] if the transaction is a contract creation.
    fn kind(&self) -> &TransactionKind;

    /// Returns a measurement of the heap usage of this type and all its internals.
    fn size(&self) -> usize;

    /// Returns the transaction type
    fn tx_type(&self) -> u8;

    /// Returns true if the transaction is an EIP-1559 transaction.
    fn is_eip1559(&self) -> bool {
        self.tx_type() == EIP1559_TX_TYPE_ID
    }

    /// Returns the length of the rlp encoded object
    fn encoded_length(&self) -> usize;

    /// Returns chain_id
    fn chain_id(&self) -> Option<u64>;
}

/// The default [PoolTransaction] for the [Pool](crate::Pool).
///
/// This type is essentially a wrapper around [TransactionSignedEcRecovered] with additional fields
/// derived from the transaction that are frequently used by the pools for ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PooledTransaction {
    /// EcRecovered transaction info
    pub(crate) transaction: TransactionSignedEcRecovered,

    /// For EIP-1559 transactions: `max_fee_per_gas * gas_limit + tx_value`.
    /// For legacy transactions: `gas_price * gas_limit + tx_value`.
    pub(crate) cost: U256,
}

impl PooledTransaction {
    /// Create new instance of [Self].
    pub fn new(transaction: TransactionSignedEcRecovered) -> Self {
        let gas_cost = match &transaction.transaction {
            Transaction::Legacy(t) => U256::from(t.gas_price) * U256::from(t.gas_limit),
            Transaction::Eip2930(t) => U256::from(t.gas_price) * U256::from(t.gas_limit),
            Transaction::Eip1559(t) => U256::from(t.max_fee_per_gas) * U256::from(t.gas_limit),
        };
        let cost = gas_cost + U256::from(transaction.value());

        Self { transaction, cost }
    }

    /// Return the reference to the underlying transaction.
    pub fn transaction(&self) -> &TransactionSignedEcRecovered {
        &self.transaction
    }
}

impl PoolTransaction for PooledTransaction {
    /// Returns hash of the transaction.
    fn hash(&self) -> &TxHash {
        self.transaction.hash_ref()
    }

    /// Returns the Sender of the transaction.
    fn sender(&self) -> Address {
        self.transaction.signer()
    }

    /// Returns the nonce for this transaction.
    fn nonce(&self) -> u64 {
        self.transaction.nonce()
    }

    /// Returns the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions: `max_fee_per_gas * gas_limit + tx_value`.
    /// For legacy transactions: `gas_price * gas_limit + tx_value`.
    fn cost(&self) -> U256 {
        self.cost
    }

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
    fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
    }

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// For legacy transactions this is gas_price.
    ///
    /// This is also commonly referred to as the "Gas Fee Cap" (`GasFeeCap`).
    fn max_fee_per_gas(&self) -> u128 {
        match &self.transaction.transaction {
            Transaction::Legacy(tx) => tx.gas_price,
            Transaction::Eip2930(tx) => tx.gas_price,
            Transaction::Eip1559(tx) => tx.max_fee_per_gas,
        }
    }

    /// Returns the EIP-1559 Priority fee the caller is paying to the block author.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match &self.transaction.transaction {
            Transaction::Legacy(_) => None,
            Transaction::Eip2930(_) => None,
            Transaction::Eip1559(tx) => Some(tx.max_priority_fee_per_gas),
        }
    }

    /// Returns the effective tip for this transaction.
    ///
    /// For EIP-1559 transactions: `min(max_fee_per_gas - base_fee, max_priority_fee_per_gas)`.
    /// For legacy transactions: `gas_price - base_fee`.
    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        self.transaction.effective_tip_per_gas(base_fee)
    }

    /// Returns the max priority fee per gas if the transaction is an EIP-1559 transaction, and
    /// otherwise returns the gas price.
    fn priority_fee_or_price(&self) -> u128 {
        self.transaction.priority_fee_or_price()
    }

    /// Returns the transaction's [`TransactionKind`], which is the address of the recipient or
    /// [`TransactionKind::Create`] if the transaction is a contract creation.
    fn kind(&self) -> &TransactionKind {
        self.transaction.kind()
    }

    /// Returns a measurement of the heap usage of this type and all its internals.
    fn size(&self) -> usize {
        self.transaction.transaction.input().len()
    }

    /// Returns the transaction type
    fn tx_type(&self) -> u8 {
        self.transaction.tx_type().into()
    }

    /// Returns the length of the rlp encoded object
    fn encoded_length(&self) -> usize {
        self.transaction.length()
    }

    /// Returns chain_id
    fn chain_id(&self) -> Option<u64> {
        self.transaction.chain_id()
    }
}

impl FromRecoveredTransaction for PooledTransaction {
    fn from_recovered_transaction(tx: TransactionSignedEcRecovered) -> Self {
        PooledTransaction::new(tx)
    }
}

impl IntoRecoveredTransaction for PooledTransaction {
    fn to_recovered_transaction(&self) -> TransactionSignedEcRecovered {
        self.transaction.clone()
    }
}

/// Represents the current status of the pool.
#[derive(Debug, Clone, Default)]
pub struct PoolSize {
    /// Number of transactions in the _pending_ sub-pool.
    pub pending: usize,
    /// Reported size of transactions in the _pending_ sub-pool.
    pub pending_size: usize,
    /// Number of transactions in the _basefee_ pool.
    pub basefee: usize,
    /// Reported size of transactions in the _basefee_ sub-pool.
    pub basefee_size: usize,
    /// Number of transactions in the _queued_ sub-pool.
    pub queued: usize,
    /// Reported size of transactions in the _queued_ sub-pool.
    pub queued_size: usize,
    /// Number of all transactions of all sub-pools
    ///
    /// Note: this is the sum of ```pending + basefee + queued```
    pub total: usize,
}

/// Represents the current status of the pool.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BlockInfo {
    /// Hash for the currently tracked block.
    pub last_seen_block_hash: H256,
    /// Current the currently tracked block.
    pub last_seen_block_number: u64,
    /// Currently enforced base fee: the threshold for the basefee sub-pool.
    ///
    /// Note: this is the derived base fee of the _next_ block that builds on the clock the pool is
    /// currently tracking.
    pub pending_basefee: u64,
}

/// A Stream that yields full transactions the subpool
#[must_use = "streams do nothing unless polled"]
#[derive(Debug)]
pub struct NewSubpoolTransactionStream<Tx: PoolTransaction> {
    st: Receiver<NewTransactionEvent<Tx>>,
    subpool: SubPool,
}

// === impl NewSubpoolTransactionStream ===

impl<Tx: PoolTransaction> NewSubpoolTransactionStream<Tx> {
    /// Create a new stream that yields full transactions from the subpool
    pub fn new(st: Receiver<NewTransactionEvent<Tx>>, subpool: SubPool) -> Self {
        Self { st, subpool }
    }
}

impl<Tx: PoolTransaction> Stream for NewSubpoolTransactionStream<Tx> {
    type Item = NewTransactionEvent<Tx>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.st.poll_recv(cx)) {
                Some(event) => {
                    if event.subpool == self.subpool {
                        return Poll::Ready(Some(event))
                    }
                }
                None => return Poll::Ready(None),
            }
        }
    }
}
