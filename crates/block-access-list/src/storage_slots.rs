use crate::StorageChange;
use alloc::vec::Vec;
use alloy_primitives::StorageKey;

/// Represents all changes made to a single storage slot across multiple transactions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SlotChanges {
    /// The storage slot key being modified.
    pub slot: StorageKey,
    /// A list of write operations to this slot, ordered by transaction index.
    pub changes: Vec<StorageChange>,
}

impl SlotChanges {
    /// Creates a new `SlotChanges` instance for the given slot key.
    ///
    /// Preallocates capacity for up to 300,000 changes.
    #[inline]
    pub fn new(slot: StorageKey) -> Self {
        Self { slot, changes: Vec::with_capacity(300_000) }
    }

    /// Appends a storage change to the list.
    #[inline]
    pub fn push(&mut self, change: StorageChange) {
        self.changes.push(change)
    }

    /// Returns `true` if no changes have been recorded.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Returns the number of changes recorded for this slot.
    #[inline]
    pub const fn len(&self) -> usize {
        self.changes.len()
    }
}
