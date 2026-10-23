use super::RevertToSlot;
use alloc::vec::Vec;
use alloy_primitives::{Address, B256, U256};
use evm2::{bytecode::Bytecode, evm::AccountInfo};

/// `accounts`/`storages`/`contracts` for inclusion into database.
///
/// Structure is made so it is easier to apply directly to database
/// that mostly have separate tables to store `accounts`/`storages`/`contracts` data.
///
/// **Note**: That data is **not** sorted. Some database benefit of faster inclusion
/// and smaller footprint if data is inserted in sorted order.
#[derive(Clone, Debug, Default)]
pub struct StateChangeset {
    /// Vector of **not** sorted accounts information.
    pub accounts: Vec<(Address, Option<AccountInfo>)>,
    /// Vector of **not** sorted storage.
    pub storage: Vec<PlainStorageChangeset>,
    /// Vector of contracts by bytecode hash. **not** sorted.
    pub contracts: Vec<(B256, Bytecode)>,
}

/// Plain storage changeset.
///
/// Used to apply storage changes of plain state to the database.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlainStorageChangeset {
    /// Address of account
    pub address: Address,
    /// Wipe storage
    pub wipe_storage: bool,
    /// Storage key value pairs
    pub storage: Vec<(U256, U256)>,
}

/// Plain Storage Revert.
///
/// [`PlainStorageRevert`] contains old values of changed storage.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlainStorageRevert {
    /// Address of account
    pub address: Address,
    /// Whether storage is wiped in this revert
    ///
    /// **Note**: Wiped flag is set on first known selfdestruct and would require clearing the
    /// state of this storage from database (And moving it to revert).
    pub wiped: bool,
    /// Contains the storage key and old values of that storage
    ///
    /// **Note**: Reverts are **not** sorted.
    pub storage_revert: Vec<(U256, RevertToSlot)>,
}

/// Plain state reverts are used to easily store reverts into database.
///
/// Note that accounts are assumed **not** sorted.
#[derive(Clone, Debug, Default)]
pub struct PlainStateReverts {
    /// Vector of account with removed contracts bytecode.
    ///
    /// **Note**: If `AccountInfo` is None means that account needs to be removed.
    pub accounts: Vec<Vec<(Address, Option<AccountInfo>)>>,
    /// Vector of storage with its address.
    pub storage: Vec<Vec<PlainStorageRevert>>,
}

impl PlainStateReverts {
    /// Constructs new [`PlainStateReverts`] with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { accounts: Vec::with_capacity(capacity), storage: Vec::with_capacity(capacity) }
    }
}
