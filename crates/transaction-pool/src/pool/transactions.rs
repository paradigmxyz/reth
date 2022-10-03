//! pool-internal transaction related types

use crate::traits::PoolTransaction;
use std::collections::HashMap;

/// Current stats about all senders and their transactions
pub struct TransactionsPerSender<T: PoolTransaction> {
    transactions: HashMap<T::Sender, TransactionSender>,

    /// How many transactions should be stored at most per sender
    max_per_peer: usize,
}

/// Current stats about one specific sender
pub struct TransactionSender {}
