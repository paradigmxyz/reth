use crate::segments::{
    AccountHistory, Receipts, ReceiptsByLogs, Segment, SenderRecovery, StorageHistory,
    TransactionLookup,
};
use reth_db::database::Database;
use reth_primitives::PruneModes;
use std::sync::Arc;

/// Collection of [Segment]. Thread-safe, allocated on the heap.
#[derive(Debug)]
pub struct SegmentSet<DB: Database> {
    inner: Vec<Arc<dyn Segment<DB>>>,
}

impl<DB: Database> SegmentSet<DB> {
    /// Returns empty [SegmentSet] collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds new [Segment] to collection.
    pub fn segment<S: Segment<DB> + 'static>(mut self, segment: S) -> Self {
        self.inner.push(Arc::new(segment));
        self
    }

    /// Adds new [Segment] to collection if it's [Some].
    pub fn segment_opt<S: Segment<DB> + 'static>(self, segment: Option<S>) -> Self {
        if let Some(segment) = segment {
            return self.segment(segment)
        }
        self
    }

    /// Consumes [SegmentSet] and returns a [Vec].
    pub fn into_vec(self) -> Vec<Arc<dyn Segment<DB>>> {
        self.inner
    }

    /// Creates a [SegmentSet] from an existing [PruneModes].
    pub fn from_prune_modes(prune_modes: PruneModes) -> Self {
        let PruneModes {
            sender_recovery,
            transaction_lookup,
            receipts,
            account_history,
            storage_history,
            receipts_log_filter,
        } = prune_modes;

        SegmentSet::default()
            // Receipts
            .segment_opt(receipts.map(Receipts::new))
            // Receipts by logs
            .segment_opt(
                (!receipts_log_filter.is_empty())
                    .then(|| ReceiptsByLogs::new(receipts_log_filter.clone())),
            )
            // Transaction lookup
            .segment_opt(transaction_lookup.map(TransactionLookup::new))
            // Sender recovery
            .segment_opt(sender_recovery.map(SenderRecovery::new))
            // Account history
            .segment_opt(account_history.map(AccountHistory::new))
            // Storage history
            .segment_opt(storage_history.map(StorageHistory::new))
    }
}

impl<DB: Database> Default for SegmentSet<DB> {
    fn default() -> Self {
        Self { inner: Vec::new() }
    }
}
