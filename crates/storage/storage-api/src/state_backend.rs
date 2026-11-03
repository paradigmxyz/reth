use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;

/// Current state kept outside the database, keyed by hashed address and hashed slot.
///
/// Only used with the hashed state layout. When a provider has one, latest-state account and
/// storage reads go to it instead of the `HashedAccounts` and `HashedStorages` tables, and the
/// provider stops writing and unwinding those tables and the state tries: the persistence hook
/// that pairs with the backend owns them.
///
/// The backend must reflect the persisted state at or after the state frontier of any open read
/// transaction. Blocks between that frontier and the backend's state are always covered by the
/// engine's in-memory overlay, so latest-state reads through it stay consistent.
///
/// Known limitation: a historical read that falls back to current state (its history lookup
/// resolves to plain state) may race a concurrent persist and observe the backend after blocks
/// its read transaction does not yet see.
pub trait StateBackend: Send + Sync + core::fmt::Debug {
    /// Returns the account at `hashed_address` in the latest persisted state.
    ///
    /// Returns `None` if the account doesn't exist.
    fn account(&self, hashed_address: B256) -> ProviderResult<Option<Account>>;

    /// Returns the storage value at `hashed_slot` of `hashed_address` in the latest persisted
    /// state.
    ///
    /// Returns `None` if the slot is unset.
    fn storage(&self, hashed_address: B256, hashed_slot: B256) -> ProviderResult<Option<U256>>;
}
