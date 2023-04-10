use crate::{error::PoolResult, pool::state::SubPool, validate::ValidPoolTransaction};
use reth_primitives::{
    Address, FromRecoveredTransaction, IntoRecoveredTransaction, PeerId, Transaction,
    TransactionKind, TransactionSignedEcRecovered, TxHash, H256, U256,
};
use reth_rlp::Encodable;
use std::{collections::HashMap, fmt, sync::Arc};
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

    /// Returns stats about the pool.
    fn status(&self) -> PoolSize;

    /// Event listener for when a new block was mined.
    ///
    /// Implementers need to update the pool accordingly.
    /// For example the base fee of the pending block is determined after a block is mined which
    /// affects the dynamic fee requirement of pending transactions in the pool.
    fn on_new_block(&self, event: OnNewBlockEvent);

    /// Imports an _external_ transaction.
    ///
    /// This is intended to be used by the network to insert incoming transactions received over the
    /// p2p network.
    ///
    /// Consumer: P2P
    async fn add_external_transaction(&self, transaction: Self::Transaction) -> PoolResult<TxHash> {
        self.add_transaction(TransactionOrigin::External, transaction).await
    }

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

    /// Returns a new Stream that yields transactions hashes for new ready transactions.
    ///
    /// Consumer: RPC
    fn pending_transactions_listener(&self) -> Receiver<TxHash>;

    /// Returns a new stream that yields new valid transactions added to the pool.
    fn transactions_listener(&self) -> Receiver<NewTransactionEvent<Self::Transaction>>;

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

    /// Removes all transactions corresponding to the given hashes.
    ///
    /// Also removes all dependent transactions.
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

/// Event fired when a new block was mined
#[derive(Debug, Clone)]
pub struct OnNewBlockEvent {
    /// Hash of the added block.
    pub hash: H256,
    /// EIP-1559 Base fee of the _next_ (pending) block
    ///
    /// The base fee of a block depends on the utilization of the last block and its base fee.
    pub pending_block_base_fee: u128,
    /// Provides a set of state changes that affected the accounts.
    pub state_changes: StateDiff,
    /// All mined transactions in the block
    pub mined_transactions: Vec<H256>,
}

/// Contains a list of changed state
#[derive(Debug, Clone)]
pub struct StateDiff {
    // TODO(mattsse) this could be an `Arc<revm::State>>`
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
pub trait PoolTransaction:
    fmt::Debug + Send + Sync + FromRecoveredTransaction + IntoRecoveredTransaction
{
    /// Hash of the transaction.
    fn hash(&self) -> &TxHash;

    /// The Sender of the transaction.
    fn sender(&self) -> Address;

    /// Returns the nonce for this transaction.
    fn nonce(&self) -> u64;

    /// Calculates the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions that is `feeCap x gasLimit + transferred_value`
    fn cost(&self) -> U256;

    /// Returns the effective gas price for this transaction.
    ///
    /// This is `priority + basefee`for EIP-1559 and `gasPrice` for legacy transactions.
    fn effective_gas_price(&self) -> u128;

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
    fn gas_limit(&self) -> u64;

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_fee_per_gas(&self) -> Option<u128>;

    /// Returns the EIP-1559 Priority fee the caller is paying to the block author.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_priority_fee_per_gas(&self) -> Option<u128>;

    /// Returns the transaction's [`TransactionKind`], which is the address of the recipient or
    /// [`TransactionKind::Create`] if the transaction is a contract creation.
    fn kind(&self) -> &TransactionKind;

    /// Returns a measurement of the heap usage of this type and all its internals.
    fn size(&self) -> usize;

    /// Returns the transaction type
    fn tx_type(&self) -> u8;

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

    /// For EIP-1559 transactions that is `feeCap x gasLimit + transferred_value
    pub(crate) cost: U256,

    /// This is `priority + basefee`for EIP-1559 and `gasPrice` for legacy transactions.
    pub(crate) effective_gas_price: u128,
}

impl PooledTransaction {
    /// Return the reference to the underlying transaction.
    pub fn transaction(&self) -> &TransactionSignedEcRecovered {
        &self.transaction
    }
}

impl PoolTransaction for PooledTransaction {
    /// Returns hash of the transaction.
    fn hash(&self) -> &TxHash {
        &self.transaction.hash
    }

    /// Returns the Sender of the transaction.
    fn sender(&self) -> Address {
        self.transaction.signer()
    }

    /// Returns the nonce for this transaction.
    fn nonce(&self) -> u64 {
        self.transaction.nonce()
    }

    /// Calculates the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions that is `feeCap x gasLimit + transferred_value`
    fn cost(&self) -> U256 {
        self.cost
    }

    /// Returns the effective gas price for this transaction.
    ///
    /// This is `priority + basefee`for EIP-1559 and `gasPrice` for legacy transactions.
    fn effective_gas_price(&self) -> u128 {
        self.effective_gas_price
    }

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
    fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
    }

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// This will return `None` for non-EIP1559 transactions
    fn max_fee_per_gas(&self) -> Option<u128> {
        match &self.transaction.transaction {
            Transaction::Legacy(_) => None,
            Transaction::Eip2930(_) => None,
            Transaction::Eip1559(tx) => Some(tx.max_fee_per_gas),
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
        let (cost, effective_gas_price) = match &tx.transaction {
            Transaction::Legacy(t) => {
                let cost = U256::from(t.gas_price) * U256::from(t.gas_limit) + U256::from(t.value);
                let effective_gas_price = t.gas_price;
                (cost, effective_gas_price)
            }
            Transaction::Eip2930(t) => {
                let cost = U256::from(t.gas_price) * U256::from(t.gas_limit) + U256::from(t.value);
                let effective_gas_price = t.gas_price;
                (cost, effective_gas_price)
            }
            Transaction::Eip1559(t) => {
                let cost =
                    U256::from(t.max_fee_per_gas) * U256::from(t.gas_limit) + U256::from(t.value);
                let effective_gas_price = t.max_priority_fee_per_gas;
                (cost, effective_gas_price)
            }
        };

        PooledTransaction { transaction: tx, cost, effective_gas_price }
    }
}

impl IntoRecoveredTransaction for PooledTransaction {
    fn to_recovered_transaction(&self) -> TransactionSignedEcRecovered {
        self.transaction.clone()
    }
}

/// Represents the current status of the pool.
#[derive(Debug, Clone)]
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
}
