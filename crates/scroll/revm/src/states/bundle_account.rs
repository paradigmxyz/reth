use crate::states::{ScrollAccountInfo, ScrollAccountInfoRevert, ScrollAccountRevert};
use reth_scroll_primitives::ScrollPostExecutionContext;
use revm::{
    db::{
        states::StorageSlot, AccountStatus, BundleAccount, RevertToSlot, StorageWithOriginalValues,
    },
    interpreter::primitives::U256,
    primitives::map::HashMap,
};

/// The scroll account bundle. Originally defined in [`BundleAccount`], a
/// scroll version of the bundle is needed for the [`crate::states::ScrollBundleState`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollBundleAccount {
    /// The current account's information
    pub info: Option<ScrollAccountInfo>,
    /// The original account's information
    pub original_info: Option<ScrollAccountInfo>,
    /// Contains both original and present state.
    /// When extracting changeset we compare if original value is different from present value.
    /// If it is different we add it to changeset.
    ///
    /// If Account was destroyed we ignore original value and compare present state with
    /// [`U256::ZERO`].
    pub storage: StorageWithOriginalValues,
    /// Account status.
    pub status: AccountStatus,
}

impl From<(BundleAccount, &ScrollPostExecutionContext)> for ScrollBundleAccount {
    fn from((account, context): (BundleAccount, &ScrollPostExecutionContext)) -> Self {
        let info = account.info.map(|info| (info, context).into());
        let original_info = account.original_info.map(|info| (info, context).into());
        Self { info, original_info, storage: account.storage, status: account.status }
    }
}

impl ScrollBundleAccount {
    /// Creates a [`ScrollBundleAccount`].
    pub const fn new(
        original_info: Option<ScrollAccountInfo>,
        present_info: Option<ScrollAccountInfo>,
        storage: StorageWithOriginalValues,
        status: AccountStatus,
    ) -> Self {
        Self { info: present_info, original_info, storage, status }
    }

    /// The approximate size of changes needed to store this account.
    /// `1 + storage_len`
    pub fn size_hint(&self) -> usize {
        1 + self.storage.len()
    }

    /// Return storage slot if it exists.
    ///
    /// In case we know that account is newly created or destroyed, return `Some(U256::ZERO)`
    pub fn storage_slot(&self, slot: U256) -> Option<U256> {
        let slot = self.storage.get(&slot).map(|s| s.present_value);
        if slot.is_some() {
            slot
        } else if self.status.is_storage_known() {
            Some(U256::ZERO)
        } else {
            None
        }
    }

    /// Fetch account info if it exists.
    pub fn account_info(&self) -> Option<ScrollAccountInfo> {
        self.info.clone()
    }

    /// Was this account destroyed.
    pub fn was_destroyed(&self) -> bool {
        self.status.was_destroyed()
    }

    /// Return true of account info was changed.
    pub fn is_info_changed(&self) -> bool {
        self.info != self.original_info
    }

    /// Return true if contract was changed
    pub fn is_contract_changed(&self) -> bool {
        self.info.as_ref().map(|a| a.code_hash) != self.original_info.as_ref().map(|a| a.code_hash)
    }

    /// Revert account to previous state and return true if account can be removed.
    pub fn revert(&mut self, revert: ScrollAccountRevert) -> bool {
        self.status = revert.previous_status;

        match revert.account {
            ScrollAccountInfoRevert::DoNothing => (),
            ScrollAccountInfoRevert::DeleteIt => {
                self.info = None;
                if self.original_info.is_none() {
                    self.storage = HashMap::default();
                    return true;
                }
                // set all storage to zero but preserve original values.
                self.storage.iter_mut().for_each(|(_, v)| {
                    v.present_value = U256::ZERO;
                });
                return false;
            }
            ScrollAccountInfoRevert::RevertTo(info) => self.info = Some(info),
        };
        // revert storage
        for (key, slot) in revert.storage {
            match slot {
                RevertToSlot::Some(value) => {
                    // Don't overwrite original values if present
                    // if storage is not present set original value as current value.
                    self.storage
                        .entry(key)
                        .or_insert_with(|| StorageSlot::new(value))
                        .present_value = value;
                }
                RevertToSlot::Destroyed => {
                    // if it was destroyed this means that storage was created and we need to remove
                    // it.
                    self.storage.remove(&key);
                }
            }
        }
        false
    }
}
