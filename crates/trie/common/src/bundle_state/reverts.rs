use super::{
    changes::PlainStorageRevert, AccountStatus, BundleAccount, PlainStateReverts,
    StorageWithOriginalValues,
};
use alloc::vec::Vec;
use alloy_primitives::{map::U256Map, Address, U256};
use core::{
    cmp::Ordering,
    ops::{Deref, DerefMut},
};
use evm2::evm::AccountInfo;

/// Contains reverts of multiple account in multiple transitions (Transitions as a block).
#[derive(Clone, Debug, Default, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Reverts(Vec<Vec<(Address, AccountRevert)>>);

impl Deref for Reverts {
    type Target = Vec<Vec<(Address, AccountRevert)>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Reverts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Reverts {
    /// Creates new reverts.
    pub const fn new(reverts: Vec<Vec<(Address, AccountRevert)>>) -> Self {
        Self(reverts)
    }

    /// Sorts account inside transition by their address.
    pub fn sort(&mut self) {
        for revert in &mut self.0 {
            revert.sort_by_key(|(address, _)| *address);
        }
    }

    /// Extends reverts with other reverts.
    pub fn extend(&mut self, other: Reverts) {
        self.0.extend(other.0);
    }

    /// Generates a [`PlainStateReverts`].
    ///
    /// Note that account are sorted by address.
    pub fn to_plain_state_reverts(&self) -> PlainStateReverts {
        let mut state_reverts = PlainStateReverts::with_capacity(self.0.len());
        for reverts in &self.0 {
            // Pessimistically pre-allocate assuming _all_ accounts changed.
            let mut accounts = Vec::with_capacity(reverts.len());
            let mut storage = Vec::with_capacity(reverts.len());
            for (address, revert_account) in reverts {
                match &revert_account.account {
                    AccountInfoRevert::RevertTo(acc) => {
                        // Cloning is cheap, because account info has 3 small
                        // fields and a Bytes
                        accounts.push((*address, Some(acc.clone())))
                    }
                    AccountInfoRevert::DeleteIt => accounts.push((*address, None)),
                    AccountInfoRevert::DoNothing => (),
                }
                if revert_account.wipe_storage || !revert_account.storage.is_empty() {
                    storage.push(PlainStorageRevert {
                        address: *address,
                        wiped: revert_account.wipe_storage,
                        storage_revert: revert_account
                            .storage
                            .iter()
                            .map(|(k, v)| (*k, *v))
                            .collect::<Vec<_>>(),
                    });
                }
            }
            state_reverts.accounts.push(accounts);
            state_reverts.storage.push(storage);
        }
        state_reverts
    }

    /// Compare two Reverts instances, ignoring the order of elements
    pub fn content_eq(&self, other: &Self) -> bool {
        if self.0.len() != other.0.len() {
            return false;
        }

        for (self_transition, other_transition) in self.0.iter().zip(other.0.iter()) {
            if self_transition.len() != other_transition.len() {
                return false;
            }

            let mut self_transition = self_transition.clone();
            let mut other_transition = other_transition.clone();
            // Sort both transitions
            self_transition.sort_by(|(addr1, revert1), (addr2, revert2)| {
                addr1.cmp(addr2).then_with(|| revert1.cmp(revert2))
            });
            other_transition.sort_by(|(addr1, revert1), (addr2, revert2)| {
                addr1.cmp(addr2).then_with(|| revert1.cmp(revert2))
            });

            // Compare sorted transitions
            if self_transition != other_transition {
                return false;
            }
        }

        true
    }

    /// Consume reverts and create [`PlainStateReverts`].
    ///
    /// Note that account are sorted by address.
    #[deprecated = "Use `to_plain_state_reverts` instead"]
    pub fn into_plain_state_reverts(self) -> PlainStateReverts {
        self.to_plain_state_reverts()
    }
}

impl PartialEq for Reverts {
    fn eq(&self, other: &Self) -> bool {
        self.content_eq(other)
    }
}

/// Assumption is that Revert can return full state from any future state to any past state.
///
/// # Note
/// It is created when new account state is applied to old account state.
///
/// And it is used to revert new account state to the old account state.
///
/// [AccountRevert] is structured in this way as we need to save it inside database.
///
/// And we need to be able to read it from database.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountRevert {
    /// Account information revert.
    pub account: AccountInfoRevert,
    /// Storage slots to revert.
    pub storage: U256Map<RevertToSlot>,
    /// Previous account status before the change.
    pub previous_status: AccountStatus,
    /// Whether to wipe the storage.
    pub wipe_storage: bool,
}

impl AccountRevert {
    /// The approximate size of changes needed to store this account revert.
    ///
    /// `1 + storage_reverts_len`
    pub fn size_hint(&self) -> usize {
        1 + self.storage.len()
    }

    /// Very similar to new_selfdestructed but it will add additional zeros
    /// ([RevertToSlot::Destroyed]) for the storage that are set if account is again created.
    pub fn new_selfdestructed_again(
        status: AccountStatus,
        account: AccountInfoRevert,
        mut previous_storage: StorageWithOriginalValues,
        updated_storage: StorageWithOriginalValues,
    ) -> Self {
        // Take present storage values as the storages that we are going to revert to.
        // As those values got destroyed.
        let mut previous_storage: U256Map<RevertToSlot> = previous_storage
            .drain()
            .map(|(key, value)| (key, RevertToSlot::Some(value.present_value)))
            .collect();
        for (key, _) in updated_storage {
            previous_storage.entry(key).or_insert(RevertToSlot::Destroyed);
        }
        AccountRevert {
            account,
            storage: previous_storage,
            previous_status: status,
            wipe_storage: false,
        }
    }

    /// Creates revert for states that were before selfdestruct.
    pub fn new_selfdestructed_from_bundle(
        account_info_revert: AccountInfoRevert,
        bundle_account: &mut BundleAccount,
        updated_storage: &StorageWithOriginalValues,
    ) -> Option<Self> {
        match bundle_account.status {
            AccountStatus::InMemoryChange |
            AccountStatus::Changed |
            AccountStatus::LoadedEmptyEIP161 |
            AccountStatus::Loaded => {
                let mut ret = AccountRevert::new_selfdestructed_again(
                    bundle_account.status,
                    account_info_revert,
                    core::mem::take(&mut bundle_account.storage),
                    updated_storage.clone(),
                );
                ret.wipe_storage = true;
                Some(ret)
            }
            _ => None,
        }
    }

    /// Create new selfdestruct revert.
    pub fn new_selfdestructed(
        status: AccountStatus,
        account: AccountInfoRevert,
        mut storage: StorageWithOriginalValues,
    ) -> Self {
        // Zero all present storage values and save present values to AccountRevert.
        let previous_storage = storage
            .iter_mut()
            .map(|(key, value)| {
                // Take previous value and set ZERO as storage got destroyed.
                (*key, RevertToSlot::Some(value.present_value))
            })
            .collect();

        Self { account, storage: previous_storage, previous_status: status, wipe_storage: true }
    }

    /// Returns `true` if there is nothing to revert,
    /// by checking that:
    /// * both account info and storage have been left untouched
    /// * we don't need to wipe storage
    pub fn is_empty(&self) -> bool {
        self.account == AccountInfoRevert::DoNothing &&
            self.storage.is_empty() &&
            !self.wipe_storage
    }
}

/// Implements partial ordering for AccountRevert
impl PartialOrd for AccountRevert {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Implements total ordering for AccountRevert
impl Ord for AccountRevert {
    fn cmp(&self, other: &Self) -> Ordering {
        // First compare accounts
        if let Some(ord) = self.account.partial_cmp(&other.account) {
            if ord != Ordering::Equal {
                return ord;
            }
        }

        // Convert HashMaps to sorted vectors for comparison
        let mut self_storage: Vec<_> = self.storage.iter().collect();
        let mut other_storage: Vec<_> = other.storage.iter().collect();

        // Sort by key and then by value
        self_storage.sort_by(|(k1, v1), (k2, v2)| k1.cmp(k2).then_with(|| v1.cmp(v2)));
        other_storage.sort_by(|(k1, v1), (k2, v2)| k1.cmp(k2).then_with(|| v1.cmp(v2)));

        // Compare each element
        for (self_entry, other_entry) in self_storage.iter().zip(other_storage.iter()) {
            let key_ord = self_entry.0.cmp(other_entry.0);
            if key_ord != Ordering::Equal {
                return key_ord;
            }
            let value_ord = self_entry.1.cmp(other_entry.1);
            if value_ord != Ordering::Equal {
                return value_ord;
            }
        }

        // If one vector is longer than the other, or if all elements are equal
        self_storage
            .len()
            .cmp(&other_storage.len())
            .then_with(|| self.previous_status.cmp(&other.previous_status))
            .then_with(|| self.wipe_storage.cmp(&other.wipe_storage))
    }
}

/// Depending on previous state of account info this
/// will tell us what to do on revert.
#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountInfoRevert {
    #[default]
    /// Nothing changed
    DoNothing,
    /// Account was created and on revert we need to remove it with all storage.
    DeleteIt,
    /// Account was changed and on revert we need to put old state.
    RevertTo(AccountInfo),
}

impl PartialOrd for AccountInfoRevert {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Orders like revm's derived ordering: by variant, then by the [`AccountInfo`] fields that its
/// `Eq` compares.
impl Ord for AccountInfoRevert {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::RevertTo(a), Self::RevertTo(b)) => {
                let order = a
                    .balance
                    .cmp(&b.balance)
                    .then_with(|| a.nonce.cmp(&b.nonce))
                    .then_with(|| a.code_hash.cmp(&b.code_hash));
                #[cfg(feature = "account-ext")]
                let order = order.then_with(|| a.extension.as_ref().cmp(b.extension.as_ref()));
                order
            }
            (Self::DoNothing, Self::DoNothing) | (Self::DeleteIt, Self::DeleteIt) => {
                Ordering::Equal
            }
            (Self::DoNothing, _) | (Self::DeleteIt, Self::RevertTo(_)) => Ordering::Less,
            (_, Self::DoNothing) | (Self::RevertTo(_), Self::DeleteIt) => Ordering::Greater,
        }
    }
}

/// So storage can have multiple types:
/// * Zero, on revert remove plain state.
/// * Value, on revert set this value
/// * Destroyed, should be removed on revert but on Revert set it as zero.
///
/// **Note**: It is completely different state if Storage is Zero or Some or if Storage was
/// Destroyed.
///
/// Because if it is destroyed, previous values can be found in database or it can be zero.
#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RevertToSlot {
    /// Revert to this value.
    Some(U256),
    /// Storage was destroyed.
    Destroyed,
}

impl RevertToSlot {
    /// Returns the previous value to set on revert.
    pub const fn to_previous_value(self) -> U256 {
        match self {
            RevertToSlot::Some(value) => value,
            RevertToSlot::Destroyed => U256::ZERO,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn content_eq_ignores_order_of_same_address() {
        let address = Address::with_last_byte(1);
        let do_nothing = AccountRevert::default();
        let delete_it =
            AccountRevert { account: AccountInfoRevert::DeleteIt, ..Default::default() };
        let a =
            Reverts::new(vec![vec![(address, do_nothing.clone()), (address, delete_it.clone())]]);
        let b = Reverts::new(vec![vec![(address, delete_it), (address, do_nothing)]]);
        assert_eq!(a, b);
    }
}
