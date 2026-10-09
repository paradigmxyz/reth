use super::{Account, EvmStorageSlot};
use core::ops::{Deref, DerefMut};
use primitives::{Address, AddressMap, StorageKey, StorageKeyMap, StorageValue};

/// EVM State is a mapping from addresses to accounts.
pub type EvmState = AddressMap<Account>;

/// An account's Storage is a mapping from 256-bit integer keys to [EvmStorageSlot]s.
pub type EvmStorage = StorageKeyMap<EvmStorageSlot>;

/// Structure used for EIP-1153 transient storage.
///
/// Maps each account [`Address`] to its transient storage slots, a mapping from
/// storage [`StorageKey`]s to [`StorageValue`]s. Transient storage is discarded
/// after every transaction.
///
/// See [EIP-1153](https://eips.ethereum.org/EIPS/eip-1153).
///
/// This is a thin wrapper around an [`AddressMap`] of [`StorageKeyMap`]s. It
/// implements [`Deref`]/[`DerefMut`] to the inner map so all map operations are
/// available, and provides [`get_value`](Self::get_value),
/// [`insert_value`](Self::insert_value) and [`remove_value`](Self::remove_value)
/// helpers for accessing individual slots without manually traversing the two
/// levels of maps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransientStorage(pub AddressMap<StorageKeyMap<StorageValue>>);

impl Deref for TransientStorage {
    type Target = AddressMap<StorageKeyMap<StorageValue>>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TransientStorage {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl TransientStorage {
    /// Returns the transient storage value for the given account `address` and
    /// storage `key`.
    ///
    /// Returns [`StorageValue::ZERO`](primitives::StorageValue) (the default) if
    /// the slot was never set.
    #[inline]
    pub fn get_value(&self, address: Address, key: StorageKey) -> StorageValue {
        self.0
            .get(&address)
            .and_then(|slots| slots.get(&key))
            .copied()
            .unwrap_or_default()
    }

    /// Inserts a transient storage `value` for the given account `address` and
    /// storage `key`, creating the account's slot map if it does not exist yet.
    ///
    /// Returns the previous value if the slot was already set.
    #[inline]
    pub fn insert_value(
        &mut self,
        address: Address,
        key: StorageKey,
        value: StorageValue,
    ) -> Option<StorageValue> {
        self.0.entry(address).or_default().insert(key, value)
    }

    /// Removes the transient storage slot for the given account `address` and
    /// storage `key`.
    ///
    /// Returns the removed value if the slot was set.
    #[inline]
    pub fn remove_value(&mut self, address: Address, key: StorageKey) -> Option<StorageValue> {
        self.0.get_mut(&address)?.remove(&key)
    }
}
