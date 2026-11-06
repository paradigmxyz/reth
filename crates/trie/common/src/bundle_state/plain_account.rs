use alloy_primitives::{
    map::{HashMap, U256Map},
    U256,
};
use evm2::evm::AccountInfo;

/// Plain account of StateDatabase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlainAccount {
    /// Account information.
    pub info: AccountInfo,
    /// Account storage.
    pub storage: PlainStorage,
}

impl PlainAccount {
    /// Creates a new empty account with the given storage.
    pub fn new_empty_with_storage(storage: PlainStorage) -> Self {
        Self { info: AccountInfo::default(), storage }
    }

    /// Consumes the account and returns its components.
    pub fn into_components(self) -> (AccountInfo, PlainStorage) {
        (self.info, self.storage)
    }
}

/// This type keeps track of the current value of a storage slot.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageSlot {
    /// The value of the storage slot before it was changed.
    ///
    /// When the slot is first loaded, this is the original value.
    ///
    /// If the slot was not changed, this is equal to the present value.
    pub previous_or_original_value: U256,
    /// When loaded with sload present value is set to original value
    pub present_value: U256,
}

impl StorageSlot {
    /// Creates a new _unchanged_ `StorageSlot` for the given value.
    pub const fn new(original: U256) -> Self {
        Self { previous_or_original_value: original, present_value: original }
    }

    /// Creates a new _changed_ `StorageSlot`.
    pub const fn new_changed(previous_or_original_value: U256, present_value: U256) -> Self {
        Self { previous_or_original_value, present_value }
    }

    /// Returns true if the present value differs from the original value
    pub fn is_changed(&self) -> bool {
        self.previous_or_original_value != self.present_value
    }

    /// Returns the original value of the storage slot.
    pub const fn original_value(&self) -> U256 {
        self.previous_or_original_value
    }

    /// Returns the current value of the storage slot.
    pub const fn present_value(&self) -> U256 {
        self.present_value
    }
}

/// This storage represent values that are before block changed.
///
/// Note: Storage that we get EVM contains original values before block changed.
pub type StorageWithOriginalValues = U256Map<StorageSlot>;

/// Simple plain storage that does not have previous value.
/// This is used for loading from database, cache and for bundle state.
pub type PlainStorage = U256Map<U256>;

impl From<AccountInfo> for PlainAccount {
    fn from(info: AccountInfo) -> Self {
        Self { info, storage: HashMap::default() }
    }
}
