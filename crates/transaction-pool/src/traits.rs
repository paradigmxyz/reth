use reth_primitives::U256;
use std::{fmt, hash::Hash};

/// General purpose abstraction fo a transaction-pool
#[async_trait::async_trait]
pub trait TransactionPool: Send + Sync {
    // TODO probably need associated `Transaction` type here
    // TODO needs transaction type

    // TODO add interfaces for adding new transactions
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
