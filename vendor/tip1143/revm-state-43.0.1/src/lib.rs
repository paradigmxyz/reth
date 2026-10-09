//! Account and storage state.
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc as std;

#[cfg(feature = "account-ext")]
mod account_extension;
mod account_info;
#[cfg(feature = "account-ext")]
pub use account_extension::AccountExtension;
pub mod bal;
mod types;

pub use bytecode;

pub use account_info::{AccountId, AccountInfo};
pub use bytecode::Bytecode;
pub use primitives;
pub use types::{EvmState, EvmStorage, TransientStorage};

use bitflags::bitflags;
use nonmax::NonMaxU32;
use primitives::{hardfork::SpecId, HashMap, StorageKey, StorageValue, U256};
use std::boxed::Box;

/// Transaction id used to track when account or storage slot was touched/loaded into the journal.
///
/// Wraps a [`NonMaxU32`] so that `Option<TransactionId>` benefits from niche optimization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TransactionId(NonMaxU32);

impl TransactionId {
    /// The zero transaction id.
    pub const ZERO: Self = Self(NonMaxU32::ZERO);

    /// Creates a new [`TransactionId`].
    ///
    /// Returns `None` if the value does not fit in the internal representation.
    #[inline]
    pub fn new(id: usize) -> Option<Self> {
        let id = u32::try_from(id).ok()?;
        NonMaxU32::new(id).map(Self)
    }

    /// Returns the transaction id as a usize.
    #[inline]
    pub const fn get(self) -> usize {
        self.0.get() as usize
    }

    /// Increments the transaction id by 1.
    ///
    /// # Panics
    ///
    /// Panics if the resulting value would equal `u32::MAX`.
    #[inline]
    pub const fn increment(&mut self) {
        self.0 = match NonMaxU32::new(self.0.get() + 1) {
            Some(id) => id,
            None => panic!("transaction id overflow"),
        };
    }
}

/// The main account type used inside Revm. It is stored inside Journal and contains all the information about the account.
///
/// Other than standard Account information it contains its status that can be both cold and warm
/// additional to that it contains BAL that is used to load data for this particular account.
///
/// On loading from database:
///     * If CompiledBal is present, load values from BAL into Account (Assume account has read data from database)
///     * In case of parallel execution, AccountInfo would be same over all parallel executions.
///     * Maybe use transaction_id as a way to notify user that this is obsolete data.
///     * Database needs to load account and tie to with BAL writes
/// If CompiledBal is not present, use loaded values
///     * Account is already up to date (uses present flow).
#[derive(Debug, Clone, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Account {
    /// Balance, nonce, and code
    pub info: AccountInfo,
    /// Transaction id, used to track when account was touched/loaded into journal.
    pub transaction_id: TransactionId,
    /// Storage cache
    pub storage: EvmStorage,
    /// Account status flags
    pub status: AccountStatus,

    /// Original account info used by BAL, changed only on cold load by BAL.
    /// `None` means `Default::default()`, to avoid allocations.
    original_info: Option<Box<AccountInfo>>,
}

impl PartialEq for Account {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.info == other.info
            && self.transaction_id == other.transaction_id
            && self.storage == other.storage
            && self.status == other.status
            && self.original_info() == other.original_info()
    }
}

impl Account {
    /// Creates new account and mark it as non existing.
    #[inline]
    pub fn new_not_existing(transaction_id: TransactionId) -> Self {
        Self {
            transaction_id,
            status: AccountStatus::LoadedAsNotExisting,
            ..Default::default()
        }
    }

    /// Make changes to the caller account.
    ///
    /// It marks the account as touched, changes the balance and bumps the nonce if `is_call` is true.
    ///
    /// Returns the old balance.
    #[inline]
    pub fn caller_initial_modification(&mut self, new_balance: U256, is_call: bool) -> U256 {
        // Touch account so we know it is changed.
        self.mark_touch();

        if is_call {
            // Nonce is already checked
            self.info.nonce = self.info.nonce.saturating_add(1);
        }

        core::mem::replace(&mut self.info.balance, new_balance)
    }

    /// Checks if account is empty and check if empty state before spurious dragon hardfork.
    #[inline]
    pub fn state_clear_aware_is_empty(&self, spec: SpecId) -> bool {
        if SpecId::is_enabled_in(spec, SpecId::SPURIOUS_DRAGON) {
            self.is_empty()
        } else {
            self.is_loaded_as_not_existing_not_touched()
        }
    }

    /// Returns the original account info.
    #[inline]
    pub fn original_info(&self) -> AccountInfo {
        self.original_info.as_deref().cloned().unwrap_or_default()
    }

    /// Returns a mutable reference to the original account info.
    #[inline]
    pub fn original_info_mut(&mut self) -> &mut AccountInfo {
        self.original_info.get_or_insert_default()
    }

    /// Clones the current info into the original info.
    pub fn set_current_info_as_original(&mut self) {
        if self.original_info.is_none() && self.info.is_default() {
            return;
        }
        self.original_info
            .get_or_insert_default()
            .as_mut()
            .clone_from(&self.info);
    }

    /// Marks the account as self destructed.
    #[inline]
    pub fn mark_selfdestruct(&mut self) {
        self.status |= AccountStatus::SelfDestructed;
    }

    /// Unmarks the account as self destructed.
    #[inline]
    pub fn unmark_selfdestruct(&mut self) {
        self.status -= AccountStatus::SelfDestructed;
    }

    /// Is account marked for self destruct.
    #[inline]
    pub const fn is_selfdestructed(&self) -> bool {
        self.status.contains(AccountStatus::SelfDestructed)
    }

    /// Marks the account as touched
    #[inline]
    pub fn mark_touch(&mut self) {
        self.status |= AccountStatus::Touched;
    }

    /// Unmarks the touch flag.
    #[inline]
    pub fn unmark_touch(&mut self) {
        self.status -= AccountStatus::Touched;
    }

    /// If account status is marked as touched.
    #[inline]
    pub const fn is_touched(&self) -> bool {
        self.status.contains(AccountStatus::Touched)
    }

    /// Returns true if account info was changed.
    #[inline]
    pub fn is_changed(&self) -> bool {
        self.original_info.as_deref().map_or_else(
            || !self.info.is_default(),
            |original| self.info != *original,
        )
    }

    /// Marks the account as newly created.
    #[inline]
    pub fn mark_created(&mut self) {
        self.status |= AccountStatus::Created;
    }

    /// Unmarks the created flag.
    #[inline]
    pub fn unmark_created(&mut self) {
        self.status -= AccountStatus::Created;
    }

    /// Marks the account as cold.
    #[inline]
    pub fn mark_cold(&mut self) {
        self.status |= AccountStatus::Cold;
    }

    /// Is account warm for given transaction id.
    #[inline]
    pub const fn is_cold_transaction_id(&self, transaction_id: TransactionId) -> bool {
        self.transaction_id.get() != transaction_id.get()
            || self.status.contains(AccountStatus::Cold)
    }

    /// Marks the account as warm and return true if it was previously cold.
    #[inline]
    pub fn mark_warm_with_transaction_id(&mut self, transaction_id: TransactionId) -> bool {
        let is_cold = self.is_cold_transaction_id(transaction_id);
        self.status -= AccountStatus::Cold;
        self.transaction_id = transaction_id;
        is_cold
    }

    /// Is account locally created
    #[inline]
    pub const fn is_created_locally(&self) -> bool {
        self.status.contains(AccountStatus::CreatedLocal)
    }

    /// Is account locally selfdestructed
    #[inline]
    pub const fn is_selfdestructed_locally(&self) -> bool {
        self.status.contains(AccountStatus::SelfDestructedLocal)
    }

    /// Selfdestruct the account by clearing its storage and resetting its account info
    #[inline]
    pub fn selfdestruct(&mut self) {
        self.storage.clear();
        self.info = AccountInfo::default();
    }

    /// Mark account as locally created and mark global created flag.
    ///
    /// Returns true if it is created globally for first time.
    #[inline]
    pub fn mark_created_locally(&mut self) -> bool {
        self.mark_local_and_global(AccountStatus::CreatedLocal, AccountStatus::Created)
    }

    /// Unmark account as locally created
    #[inline]
    pub fn unmark_created_locally(&mut self) {
        self.status -= AccountStatus::CreatedLocal;
    }

    /// Mark account as locally and globally selfdestructed
    #[inline]
    pub fn mark_selfdestructed_locally(&mut self) -> bool {
        self.mark_local_and_global(
            AccountStatus::SelfDestructedLocal,
            AccountStatus::SelfDestructed,
        )
    }

    #[inline]
    fn mark_local_and_global(
        &mut self,
        local_flag: AccountStatus,
        global_flag: AccountStatus,
    ) -> bool {
        self.status |= local_flag;
        let is_global_first_time = !self.status.contains(global_flag);
        self.status |= global_flag;
        is_global_first_time
    }

    /// Unmark account as locally selfdestructed
    #[inline]
    pub fn unmark_selfdestructed_locally(&mut self) {
        self.status -= AccountStatus::SelfDestructedLocal;
    }

    /// Is account loaded as not existing from database.
    ///
    /// This is needed for pre spurious dragon hardforks where
    /// existing and empty were two separate states.
    pub const fn is_loaded_as_not_existing(&self) -> bool {
        self.status.contains(AccountStatus::LoadedAsNotExisting)
    }

    /// Is account loaded as not existing from database and not touched.
    pub const fn is_loaded_as_not_existing_not_touched(&self) -> bool {
        self.is_loaded_as_not_existing() && !self.is_touched()
    }

    /// Is account newly created in this transaction.
    pub const fn is_created(&self) -> bool {
        self.status.contains(AccountStatus::Created)
    }

    /// Is account empty, check if nonce and balance are zero and code is empty.
    pub fn is_empty(&self) -> bool {
        self.info.is_empty()
    }

    /// Returns an iterator over the storage slots that have been changed.
    ///
    /// See also [EvmStorageSlot::is_changed].
    pub fn changed_storage_slots(&self) -> impl Iterator<Item = (&StorageKey, &EvmStorageSlot)> {
        self.storage.iter().filter(|(_, slot)| slot.is_changed())
    }

    /// Sets account info and returns self for method chaining.
    pub fn with_info(mut self, info: AccountInfo) -> Self {
        self.info = info;
        self
    }

    /// Populates storage from an iterator of storage slots and returns self for method chaining.
    pub fn with_storage<I>(mut self, storage_iter: I) -> Self
    where
        I: Iterator<Item = (StorageKey, EvmStorageSlot)>,
    {
        for (key, slot) in storage_iter {
            self.storage.insert(key, slot);
        }
        self
    }

    /// Marks the account as self destructed and returns self for method chaining.
    pub fn with_selfdestruct_mark(mut self) -> Self {
        self.mark_selfdestruct();
        self
    }

    /// Marks the account as touched and returns self for method chaining.
    pub fn with_touched_mark(mut self) -> Self {
        self.mark_touch();
        self
    }

    /// Marks the account as newly created and returns self for method chaining.
    pub fn with_created_mark(mut self) -> Self {
        self.mark_created();
        self
    }

    /// Marks the account as cold and returns self for method chaining.
    pub fn with_cold_mark(mut self) -> Self {
        self.mark_cold();
        self
    }

    /// Marks the account as warm (not cold) and returns self for method chaining.
    /// Also returns whether the account was previously cold.
    pub fn with_warm_mark(mut self, transaction_id: TransactionId) -> (Self, bool) {
        let was_cold = self.mark_warm_with_transaction_id(transaction_id);
        (self, was_cold)
    }

    /// Variant of with_warm_mark that doesn't return the previous state.
    pub fn with_warm(mut self, transaction_id: TransactionId) -> Self {
        self.mark_warm_with_transaction_id(transaction_id);
        self
    }
}

impl From<AccountInfo> for Account {
    fn from(info: AccountInfo) -> Self {
        let original_info = if info.is_default() {
            None
        } else {
            Some(Box::new(info.clone()))
        };
        Self {
            info,
            original_info,
            transaction_id: TransactionId::ZERO,
            storage: HashMap::default(),
            status: AccountStatus::empty(),
        }
    }
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::*;
    use serde::Deserialize;

    /// Distinguishes missing field (old format) from explicit `null` (new format).
    #[derive(Default)]
    enum MaybeOriginalInfo {
        /// Field was missing from JSON (old format).
        #[default]
        Missing,
        /// Present in JSON: `null` means default, `Some` is the value.
        Present(Option<AccountInfo>),
    }

    impl<'de> Deserialize<'de> for MaybeOriginalInfo {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            Option::<AccountInfo>::deserialize(deserializer).map(MaybeOriginalInfo::Present)
        }
    }

    #[derive(Deserialize)]
    // Field order must match `Account`'s declaration order: the derived `Serialize` emits
    // fields positionally, and non-self-describing formats (bincode, postcard) replay them in
    // this struct's declared order — only self-describing formats match fields by name.
    struct AccountSerde {
        info: AccountInfo,
        transaction_id: TransactionId,
        storage: EvmStorage,
        status: AccountStatus,
        #[serde(default)]
        original_info: MaybeOriginalInfo,
    }

    impl<'de> Deserialize<'de> for super::Account {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let AccountSerde {
                info,
                original_info,
                storage,
                transaction_id,
                status,
            } = Deserialize::deserialize(deserializer)?;

            let original_info = match original_info {
                // Old format: field missing → use info as original.
                MaybeOriginalInfo::Missing => Some(Box::new(info.clone())),
                // New format: null → None (default).
                MaybeOriginalInfo::Present(None) => None,
                // New format: explicit value.
                MaybeOriginalInfo::Present(Some(oi)) => Some(Box::new(oi)),
            };

            Ok(Account {
                info,
                original_info,
                storage,
                transaction_id,
                status,
            })
        }
    }
}

// The `bitflags!` macro generates `struct`s that manage a set of flags.
bitflags! {
    /// Account status flags. Generated by bitflags crate.
    ///
    /// With multi transaction feature there is a need to have both global and local fields.
    /// Global across multiple transaction and local across one transaction execution.
    ///
    /// Empty state without any flags set represent account that is loaded from db but not interacted with.
    ///
    /// `Touched` flag is used by database to check if account is potentially changed in some way.
    /// Additionally, after EIP-161 touch on empty-existing account would remove this account from state
    /// after transaction execution ends. Touch can span across multiple transactions as it is needed
    /// to be marked only once so it is safe to have only one global flag.
    /// Only first touch have different behaviour from others, and touch in first transaction will invalidate
    /// touch functionality in next transactions.
    ///
    /// `Created` flag is used to mark account as newly created in this transaction. This is used for optimization
    /// where if this flag is set we will not access database to fetch storage values.
    ///
    /// `CreatedLocal` flag is used after cancun to enable selfdestruct cleanup if account is created in same transaction.
    ///
    /// `Selfdestructed` flag is used to mark account as selfdestructed. On multiple calls this flag is preserved
    /// and on revert will stay selfdestructed.
    ///
    /// `SelfdestructLocal` is needed to award refund on first selfdestruct call. This flag is cleared on account loading.
    /// Over multiple transaction account can be selfdestructed in one tx, created in second tx and selfdestructed again in
    /// third tx.
    /// Additionally if account is loaded in second tx, storage and account that was destroyed in first tx needs to be cleared.
    ///
    /// `LoadedAsNotExisting` is used to mark account as loaded from database but with `balance == 0 && nonce == 0 && code = 0x`.
    /// This flag is fine to span across multiple transactions as it interucts with `Touched` flag this is used in global scope.
    ///
    /// `CreatedLocal`, `SelfdestructedLocal` and `Cold` flags are reset on first account loading of local scope.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "serde", serde(transparent))]
    pub struct AccountStatus: u8 {
        /// When account is newly created we will not access database
        /// to fetch storage values.
        const Created = 0b00000001;
        /// When accounts gets loaded this flag is set to false. Create will always be true if CreatedLocal is true.
        const CreatedLocal = 0b10000000;
        /// If account is marked for self destruction.
        const SelfDestructed = 0b00000010;
        /// If account is marked for self destruction.
        const SelfDestructedLocal = 0b01000000;
        /// Only when account is marked as touched we will save it to database.
        /// Additionally first touch on empty existing account (After EIP-161) will mark it
        /// for removal from state after transaction execution.
        const Touched = 0b00000100;
        /// used only for pre spurious dragon hardforks where existing and empty were two separate states.
        /// it became same state after EIP-161: State trie clearing
        const LoadedAsNotExisting = 0b00001000;
        /// used to mark account as cold.
        /// It is used only in local scope and it is reset on account loading.
        const Cold = 0b00010000;
    }
}

impl AccountStatus {
    /// Returns true if the account status is touched.
    #[inline]
    pub const fn is_touched(&self) -> bool {
        self.contains(AccountStatus::Touched)
    }
}

impl Default for AccountStatus {
    fn default() -> Self {
        AccountStatus::empty()
    }
}

/// This type keeps track of the current value of a storage slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EvmStorageSlot {
    /// Original value of the storage slot
    pub original_value: StorageValue,
    /// Present value of the storage slot
    pub present_value: StorageValue,
    /// Transaction id, used to track when storage slot was made warm.
    pub transaction_id: TransactionId,
    /// Represents if the storage slot is cold
    pub is_cold: bool,
}

impl EvmStorageSlot {
    /// Creates a new _unchanged_ `EvmStorageSlot` for the given value.
    pub const fn new(original: StorageValue, transaction_id: TransactionId) -> Self {
        Self {
            original_value: original,
            present_value: original,
            transaction_id,
            is_cold: false,
        }
    }

    /// Creates a new _changed_ `EvmStorageSlot`.
    pub const fn new_changed(
        original_value: StorageValue,
        present_value: StorageValue,
        transaction_id: TransactionId,
    ) -> Self {
        Self {
            original_value,
            present_value,
            transaction_id,
            is_cold: false,
        }
    }
    /// Returns true if the present value differs from the original value.
    pub fn is_changed(&self) -> bool {
        self.original_value != self.present_value
    }

    /// Returns the original value of the storage slot.
    #[inline]
    pub const fn original_value(&self) -> StorageValue {
        self.original_value
    }

    /// Returns the current value of the storage slot.
    #[inline]
    pub const fn present_value(&self) -> StorageValue {
        self.present_value
    }

    /// Marks the storage slot as cold. Does not change transaction_id.
    #[inline]
    pub const fn mark_cold(&mut self) {
        self.is_cold = true;
    }

    /// Is storage slot cold for given transaction id.
    #[inline]
    pub const fn is_cold_transaction_id(&self, transaction_id: TransactionId) -> bool {
        self.transaction_id.get() != transaction_id.get() || self.is_cold
    }

    /// Marks the storage slot as warm and sets transaction_id to the given value
    ///
    ///
    /// Returns false if old transition_id is different from given id or in case they are same return `Self::is_cold` value.
    #[inline]
    pub const fn mark_warm_with_transaction_id(&mut self, transaction_id: TransactionId) -> bool {
        let is_cold = self.is_cold_transaction_id(transaction_id);
        // Re-baseline the EIP-2200 `original_value` only when the slot belongs to a *previous*
        // transaction (transaction id mismatch). `is_cold` also covers a slot flagged cold
        // within the same transaction (e.g. a reverted `StorageWarmed` entry, or an explicit
        // `mark_cold`); those must keep the original value captured at the start of this
        // transaction. The committed/original value is tx-scoped, not access-list-scoped.
        if self.transaction_id.get() != transaction_id.get() {
            self.original_value = self.present_value;
        }
        self.transaction_id = transaction_id;
        self.is_cold = false;
        is_cold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvmStorageSlot;
    use primitives::{StorageKey, KECCAK_EMPTY, U256};

    #[test]
    fn account_is_empty_balance() {
        let mut account = Account::default();
        assert!(account.is_empty());

        account.info.balance = U256::from(1);
        assert!(!account.is_empty());

        account.info.balance = U256::ZERO;
        assert!(account.is_empty());
    }

    #[test]
    fn account_is_empty_nonce() {
        let mut account = Account::default();
        assert!(account.is_empty());

        account.info.nonce = 1;
        assert!(!account.is_empty());

        account.info.nonce = 0;
        assert!(account.is_empty());
    }

    #[test]
    fn account_is_empty_code_hash() {
        let mut account = Account::default();
        assert!(account.is_empty());

        account.info.code_hash = [1; 32].into();
        assert!(!account.is_empty());

        account.info.code_hash = [0; 32].into();
        assert!(account.is_empty());

        account.info.code_hash = KECCAK_EMPTY;
        assert!(account.is_empty());
    }

    #[test]
    fn account_state() {
        let mut account = Account::default();

        assert!(!account.is_touched());
        assert!(!account.is_selfdestructed());

        account.mark_touch();
        assert!(account.is_touched());
        assert!(!account.is_selfdestructed());

        account.mark_selfdestruct();
        assert!(account.is_touched());
        assert!(account.is_selfdestructed());

        account.unmark_selfdestruct();
        assert!(account.is_touched());
        assert!(!account.is_selfdestructed());
    }

    #[test]
    fn account_is_cold() {
        let mut account = Account::default();

        // Account is not cold by default
        assert!(!account.status.contains(crate::AccountStatus::Cold));

        // When marking warm account as warm again, it should return false
        assert!(!account.mark_warm_with_transaction_id(TransactionId::ZERO));

        // Mark account as cold
        account.mark_cold();

        // Account is cold
        assert!(account.status.contains(crate::AccountStatus::Cold));

        // When marking cold account as warm, it should return true
        assert!(account.mark_warm_with_transaction_id(TransactionId::ZERO));
    }

    #[test]
    fn test_account_with_info() {
        let info = AccountInfo::default();
        let account = Account::default().with_info(info.clone());

        assert_eq!(account.info, info);
        assert_eq!(account.storage, HashMap::default());
        assert_eq!(account.status, AccountStatus::empty());
    }

    #[test]
    fn test_account_with_storage() {
        let mut storage = HashMap::<StorageKey, EvmStorageSlot>::default();
        let key1 = StorageKey::from(1);
        let key2 = StorageKey::from(2);
        let slot1 = EvmStorageSlot::new(StorageValue::from(10), TransactionId::ZERO);
        let slot2 = EvmStorageSlot::new(StorageValue::from(20), TransactionId::ZERO);

        storage.insert(key1, slot1.clone());
        storage.insert(key2, slot2.clone());

        let account = Account::default().with_storage(storage.clone().into_iter());

        assert_eq!(account.storage.len(), 2);
        assert_eq!(account.storage.get(&key1), Some(&slot1));
        assert_eq!(account.storage.get(&key2), Some(&slot2));
    }

    #[test]
    fn test_account_with_selfdestruct_mark() {
        let account = Account::default().with_selfdestruct_mark();

        assert!(account.is_selfdestructed());
        assert!(!account.is_touched());
        assert!(!account.is_created());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_account_serialize_deserialize() {
        let account = Account::default().with_selfdestruct_mark();
        let serialized = serde_json::to_string(&account).unwrap();
        let deserialized: Account = serde_json::from_str(&serialized).unwrap();
        assert_eq!(account, deserialized);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_account_binary_round_trip() {
        // Positional formats carry no field names, so a `Serialize`/`Deserialize` field-order
        // mismatch is invisible to the JSON tests above and a default account round-trips by
        // accident. Populate every field with a distinct non-default value, `original_info`
        // differing from `info`, to pin each field's position.
        let mut account = Account::from(AccountInfo {
            nonce: 5,
            ..AccountInfo::default()
        });
        account.info.nonce = 7;
        account.transaction_id = TransactionId::new(3).unwrap();
        account.storage.insert(
            StorageKey::from(42u64),
            EvmStorageSlot {
                original_value: U256::from(1u64),
                present_value: U256::from(2u64),
                transaction_id: account.transaction_id,
                is_cold: false,
            },
        );
        account.status = AccountStatus::Touched;

        // MessagePack delimits structs, so omitted account extensions cannot consume later fields.
        #[cfg(feature = "account-ext")]
        let decoded: Account =
            rmp_serde::from_slice(&rmp_serde::to_vec(&account).unwrap()).unwrap();
        #[cfg(not(feature = "account-ext"))]
        let decoded: Account =
            postcard::from_bytes(&postcard::to_allocvec(&account).unwrap()).unwrap();

        assert_eq!(account, decoded);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_account_original_info_none_roundtrip() {
        let account = Account::new_not_existing(TransactionId::new(2).unwrap());
        assert!(account.original_info.is_none());
        let serialized = serde_json::to_string(&account).unwrap();
        let deserialized: Account = serde_json::from_str(&serialized).unwrap();
        assert!(deserialized.original_info.is_none());
        assert_eq!(account, deserialized);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_account_deserialize_original_info_missing_null_present() {
        let code = r#"{"LegacyAnalyzed":{"bytecode":"0x00","original_len":0,"jump_table":{"order":"bitvec::order::Lsb0","head":{"width":8,"index":0},"bits":0,"data":[]}}}"#;
        let info = format!(
            r#"{{"balance":"0x2386f26fc10000","nonce":1,"code_hash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","code":{code}}}"#
        );

        // Missing field (old format): original_info = Some(info.clone()).
        let json =
            format!(r#"{{"info":{info},"transaction_id":0,"storage":{{}},"status":"Touched"}}"#);
        let acct: Account = serde_json::from_str(&json).unwrap();
        assert!(acct.original_info.is_some());
        assert_eq!(acct.original_info(), acct.info);

        // Null (new format): original_info = None (default).
        let json = format!(
            r#"{{"info":{info},"original_info":null,"transaction_id":0,"storage":{{}},"status":"Touched"}}"#
        );
        let acct: Account = serde_json::from_str(&json).unwrap();
        assert!(acct.original_info.is_none());

        // Present value.
        let original = r#"{"balance":"0x0","nonce":0,"code_hash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","code":null}"#;
        let json = format!(
            r#"{{"info":{info},"original_info":{original},"transaction_id":0,"storage":{{}},"status":"Touched"}}"#
        );
        let acct: Account = serde_json::from_str(&json).unwrap();
        assert!(acct.original_info.is_some());
        assert_eq!(acct.original_info().nonce, 0);
        assert_eq!(acct.original_info().balance, U256::ZERO);
    }

    #[test]
    fn test_account_with_touched_mark() {
        let account = Account::default().with_touched_mark();

        assert!(!account.is_selfdestructed());
        assert!(account.is_touched());
        assert!(!account.is_created());
    }

    #[test]
    fn test_account_with_created_mark() {
        let account = Account::default().with_created_mark();

        assert!(!account.is_selfdestructed());
        assert!(!account.is_touched());
        assert!(account.is_created());
    }

    #[test]
    fn test_account_with_cold_mark() {
        let account = Account::default().with_cold_mark();

        assert!(account.status.contains(AccountStatus::Cold));
    }

    #[test]
    fn test_storage_mark_warm_with_transaction_id() {
        let tx_zero = TransactionId::ZERO;
        let tx_one = TransactionId::new(1).unwrap();
        let mut slot = EvmStorageSlot::new(U256::ZERO, tx_zero);
        slot.is_cold = true;
        slot.transaction_id = tx_zero;
        assert!(slot.mark_warm_with_transaction_id(tx_one));

        slot.is_cold = false;
        slot.transaction_id = tx_zero;
        assert!(slot.mark_warm_with_transaction_id(tx_one));

        slot.is_cold = true;
        slot.transaction_id = tx_one;
        assert!(slot.mark_warm_with_transaction_id(tx_one));

        slot.is_cold = false;
        slot.transaction_id = tx_one;
        // Only if transaction id is same and is_cold is false, return false.
        assert!(!slot.mark_warm_with_transaction_id(tx_one));
    }

    #[test]
    fn test_account_with_warm_mark() {
        // Start with a cold account
        let cold_account = Account::default().with_cold_mark();
        assert!(cold_account.status.contains(AccountStatus::Cold));

        // Use with_warm_mark to warm it
        let (warm_account, was_cold) = cold_account.with_warm_mark(TransactionId::ZERO);

        // Check that it's now warm and previously was cold
        assert!(!warm_account.status.contains(AccountStatus::Cold));
        assert!(was_cold);

        // Try with an already warm account
        let (still_warm_account, was_cold) = warm_account.with_warm_mark(TransactionId::ZERO);
        assert!(!still_warm_account.status.contains(AccountStatus::Cold));
        assert!(!was_cold);
    }

    #[test]
    fn test_account_with_warm() {
        // Start with a cold account
        let cold_account = Account::default().with_cold_mark();
        assert!(cold_account.status.contains(AccountStatus::Cold));

        // Use with_warm to warm it
        let warm_account = cold_account.with_warm(TransactionId::ZERO);

        // Check that it's now warm
        assert!(!warm_account.status.contains(AccountStatus::Cold));
    }

    #[test]
    fn test_account_builder_chaining() {
        let info = AccountInfo {
            nonce: 5,
            ..AccountInfo::default()
        };

        let slot_key = StorageKey::from(42);
        let slot_value = EvmStorageSlot::new(StorageValue::from(123), TransactionId::ZERO);
        let mut storage = HashMap::<StorageKey, EvmStorageSlot>::default();
        storage.insert(slot_key, slot_value.clone());

        // Chain multiple builder methods together
        let account = Account::default()
            .with_info(info.clone())
            .with_storage(storage.into_iter())
            .with_created_mark()
            .with_touched_mark()
            .with_cold_mark()
            .with_warm(TransactionId::ZERO);

        // Verify all modifications were applied
        assert_eq!(account.info, info);
        assert_eq!(account.storage.get(&slot_key), Some(&slot_value));
        assert!(account.is_created());
        assert!(account.is_touched());
        assert!(!account.status.contains(AccountStatus::Cold));
    }

    #[test]
    fn test_account_is_cold_transaction_id() {
        let tx_zero = TransactionId::ZERO;
        let tx_one = TransactionId::new(1).unwrap();
        let mut account = Account::default();
        // only case where it is warm.
        assert!(!account.is_cold_transaction_id(tx_zero));

        // all other cases are cold
        assert!(account.is_cold_transaction_id(tx_one));
        account.mark_cold();
        assert!(account.is_cold_transaction_id(tx_zero));
        assert!(account.is_cold_transaction_id(tx_one));
    }
}
