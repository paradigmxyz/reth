use alloy_primitives::{StorageValue, TxIndex};

/// Represents a single storage write operation within a transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageChange {
    /// Index of the transaction that performed the write.
    pub tx_index: TxIndex,
    /// The new value written to the storage slot.
    pub new_value: StorageValue,
}

impl StorageChange {
    /// Creates a new `StorageChange`.
    #[inline]
    pub const fn new(tx_index: TxIndex, new_value: StorageValue) -> Self {
        Self { tx_index, new_value }
    }

    /// Returns true if the new value is zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.new_value == StorageValue::ZERO
    }

    /// Returns true if this change was made by the given transaction.
    #[inline]
    pub const fn is_from_tx(&self, tx: TxIndex) -> bool {
        self.tx_index == tx
    }

    /// Returns a copy with a different storage value.
    #[inline]
    pub const fn with_value(&self, value: StorageValue) -> Self {
        Self { tx_index: self.tx_index, new_value: value }
    }
}
