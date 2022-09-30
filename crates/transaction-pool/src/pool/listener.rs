//! Listeners for the transaction-pool

use crate::pool::events::TransactionEvent;
use futures::channel::mpsc::UnboundedSender;
use std::{collections::HashMap, hash};

/// Transaction pool event listeners.
pub struct PoolEventListener<Hash: hash::Hash + Eq, BlockHash> {
    /// All listeners for certain transactions.
    listeners: HashMap<Hash, UnboundedSender<PoolEventListenerSender<Hash, BlockHash>>>,
}

/// Sender half(s) of the event channels for a specific transaction
#[derive(Debug)]
pub struct PoolEventListenerSender<Hash, BlockHash> {
    /// Corresponding receiver half(s) for the transaction
    receivers: Vec<UnboundedSender<TransactionEvent<Hash, BlockHash>>>,
}
