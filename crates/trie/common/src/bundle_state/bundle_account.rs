use super::{
    reverts::AccountInfoRevert, AccountRevert, AccountStatus, RevertToSlot, StorageSlot,
    StorageWithOriginalValues, TransitionAccount,
};
use alloy_primitives::{
    map::{HashMap, U256Map},
    U256,
};
use evm2::evm::AccountInfo;

/// Account information focused on creating of database changesets
/// and Reverts.
///
/// Status is needed as to know from what state we are applying the TransitionAccount.
///
/// Original account info is needed to know if there was a change.
///
/// Same thing for storage with original value.
///
/// On selfdestruct storage original value is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BundleAccount {
    /// Current account information.
    pub info: Option<AccountInfo>,
    /// Original account information before modifications.
    pub original_info: Option<AccountInfo>,
    /// Contains both original and present state.
    /// When extracting changeset we compare if original value is different from present value.
    /// If it is different we add it to changeset.
    ///
    /// If Account was destroyed we ignore original value and compare present state with
    /// U256::ZERO.
    pub storage: StorageWithOriginalValues,
    /// Account status.
    pub status: AccountStatus,
}

impl BundleAccount {
    /// Create new BundleAccount.
    pub const fn new(
        original_info: Option<AccountInfo>,
        present_info: Option<AccountInfo>,
        storage: StorageWithOriginalValues,
        status: AccountStatus,
    ) -> Self {
        Self { info: present_info, original_info, storage, status }
    }

    /// The approximate size of changes needed to store this account.
    ///
    /// `1 + storage_len`
    pub fn size_hint(&self) -> usize {
        1 + self.storage.len()
    }

    /// Return storage slot if it exists.
    ///
    /// In case we know that account is newly created or destroyed, return
    /// `Some(U256::ZERO)`
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
    pub fn account_info(&self) -> Option<AccountInfo> {
        self.info.clone()
    }

    /// Was this account destroyed.
    pub const fn was_destroyed(&self) -> bool {
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
    pub fn revert(&mut self, revert: AccountRevert) -> bool {
        self.status = revert.previous_status;

        match revert.account {
            AccountInfoRevert::DoNothing => (),
            AccountInfoRevert::DeleteIt => {
                self.info = None;
                if self.original_info.is_none() {
                    self.storage = HashMap::default();
                    return true;
                } else {
                    // Set all storage to zero but preserve original values.
                    self.storage.iter_mut().for_each(|(_, v)| {
                        v.present_value = U256::ZERO;
                    });
                    return false;
                }
            }
            AccountInfoRevert::RevertTo(info) => self.info = Some(info),
        };
        // Revert storage
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
                    // If it was destroyed this means that storage was created and we need to remove
                    // it.
                    self.storage.remove(&key);
                }
            }
        }
        false
    }

    /// Update to new state and generate AccountRevert that if applied to new state will
    /// revert it to previous state.
    ///
    /// If no revert is present, update is noop.
    pub fn update_and_create_revert(
        &mut self,
        transition: TransitionAccount,
    ) -> Option<AccountRevert> {
        let updated_info = transition.info;
        let updated_storage = transition.storage;
        let updated_status = transition.status;

        // The helper that extends this storage but preserves original value.
        let extend_storage =
            |this_storage: &mut StorageWithOriginalValues,
             storage_update: StorageWithOriginalValues| {
                this_storage.reserve(storage_update.len());
                for (key, value) in storage_update {
                    this_storage.entry(key).or_insert(value).present_value = value.present_value;
                }
            };

        let previous_storage_from_update =
            |updated_storage: &StorageWithOriginalValues| -> U256Map<RevertToSlot> {
                updated_storage
                    .iter()
                    .filter(|s| s.1.is_changed())
                    .map(|(key, value)| {
                        (*key, RevertToSlot::Some(value.previous_or_original_value))
                    })
                    .collect()
            };

        // Needed for some reverts.
        let info_revert = if self.info != updated_info {
            AccountInfoRevert::RevertTo(self.info.clone().unwrap_or_default())
        } else {
            AccountInfoRevert::DoNothing
        };

        let account_revert = match updated_status {
            AccountStatus::Changed => {
                let previous_storage = previous_storage_from_update(&updated_storage);
                match self.status {
                    AccountStatus::Changed | AccountStatus::Loaded => {
                        // Extend the storage. original values is not used inside bundle.
                        extend_storage(&mut self.storage, updated_storage);
                    }
                    AccountStatus::LoadedEmptyEIP161 => {
                        // Do nothing.
                        // Only change that can happen from LoadedEmpty to Changed is if balance
                        // is send to account. So we are only checking account change here.
                    }
                    _ => unreachable!("Invalid state transfer to Changed from {self:?}"),
                };
                let previous_status = self.status;
                self.status = AccountStatus::Changed;
                self.info = updated_info;
                Some(AccountRevert {
                    account: info_revert,
                    storage: previous_storage,
                    previous_status,
                    wipe_storage: false,
                })
            }
            AccountStatus::InMemoryChange => {
                let previous_storage = previous_storage_from_update(&updated_storage);
                let in_memory_info_revert = match self.status {
                    AccountStatus::Loaded | AccountStatus::InMemoryChange => {
                        // From loaded (Or LoadedEmpty) to InMemoryChange can happen if there is
                        // balance change or new created account but Loaded
                        // didn't have contract.
                        extend_storage(&mut self.storage, updated_storage);
                        info_revert
                    }
                    AccountStatus::LoadedEmptyEIP161 => {
                        self.storage = updated_storage;
                        info_revert
                    }
                    AccountStatus::LoadedNotExisting => {
                        self.storage = updated_storage;
                        AccountInfoRevert::DeleteIt
                    }
                    _ => unreachable!("Invalid change to InMemoryChange from {self:?}"),
                };
                let previous_status = self.status;
                self.status = AccountStatus::InMemoryChange;
                self.info = updated_info;
                Some(AccountRevert {
                    account: in_memory_info_revert,
                    storage: previous_storage,
                    previous_status,
                    wipe_storage: false,
                })
            }
            AccountStatus::Loaded |
            AccountStatus::LoadedNotExisting |
            AccountStatus::LoadedEmptyEIP161 => {
                // No changeset, maybe just update data
                // Do nothing for now.
                None
            }
            AccountStatus::Destroyed => {
                // Clear this storage and move it to the Revert.
                let this_storage = core::mem::take(&mut self.storage);
                let ret = match self.status {
                    AccountStatus::InMemoryChange | AccountStatus::Changed | AccountStatus::Loaded | AccountStatus::LoadedEmptyEIP161 => {
                        Some(AccountRevert::new_selfdestructed(self.status, info_revert, this_storage))
                    }
                    AccountStatus::LoadedNotExisting => {
                        // Do nothing as we have LoadedNotExisting -> Destroyed (It is noop)
                        None
                    }
                    _ => unreachable!("Invalid transition to Destroyed account from: {self:?} to {updated_info:?} {updated_status:?}"),
                };

                if ret.is_some() {
                    self.status = AccountStatus::Destroyed;
                    self.info = None;
                }

                // Set present to destroyed.
                ret
            }
            AccountStatus::DestroyedChanged => {
                // Previous block created account or changed.
                // (It was destroyed on previous block or one before).

                // Check common pre destroy paths.
                // If common path is there it will drain the storage.
                if let Some(revert_state) = AccountRevert::new_selfdestructed_from_bundle(
                    info_revert.clone(),
                    self,
                    &updated_storage,
                ) {
                    // Set to destroyed and revert state.
                    self.status = AccountStatus::DestroyedChanged;
                    self.info = updated_info;
                    self.storage = updated_storage;

                    Some(revert_state)
                } else {
                    let ret = match self.status {
                        AccountStatus::Destroyed | AccountStatus::LoadedNotExisting => {
                            // From destroyed state new account is made
                            Some(AccountRevert {
                                account: AccountInfoRevert::DeleteIt,
                                storage: previous_storage_from_update(&updated_storage),
                                previous_status: self.status,
                                wipe_storage: false,
                            })
                        }
                        AccountStatus::DestroyedChanged => {
                            // Account was destroyed in this transition. So we should clear present
                            // storage and insert it inside revert.

                            let previous_storage = if transition.storage_was_destroyed {
                                let mut storage = core::mem::take(&mut self.storage)
                                    .into_iter()
                                    .map(|t| (t.0, RevertToSlot::Some(t.1.present_value)))
                                    .collect::<U256Map<_>>();
                                for key in updated_storage.keys() {
                                    // As it is not existing inside Destroyed storage this means
                                    // that previous values must be zero
                                    storage.entry(*key).or_insert(RevertToSlot::Destroyed);
                                }
                                storage
                            } else {
                                previous_storage_from_update(&updated_storage)
                            };

                            Some(AccountRevert {
                                account: info_revert,
                                storage: previous_storage,
                                previous_status: AccountStatus::DestroyedChanged,
                                wipe_storage: false,
                            })
                        }
                        AccountStatus::DestroyedAgain => {
                            Some(AccountRevert::new_selfdestructed_again(
                                // Destroyed again will set empty account.
                                AccountStatus::DestroyedAgain,
                                AccountInfoRevert::DeleteIt,
                                HashMap::default(),
                                updated_storage.clone(),
                            ))
                        }
                        _ => unreachable!("Invalid state transfer to DestroyedNew from {self:?}"),
                    };
                    self.status = AccountStatus::DestroyedChanged;
                    self.info = updated_info;
                    // Extends current storage.
                    extend_storage(&mut self.storage, updated_storage);

                    ret
                }
            }
            AccountStatus::DestroyedAgain => {
                // Previous block created account
                // (It was destroyed on previous block or one before).

                // Check common pre destroy paths.
                // This will drain the storage if it is common transition.
                let ret = if let Some(revert_state) = AccountRevert::new_selfdestructed_from_bundle(
                    info_revert,
                    self,
                    &HashMap::default(),
                ) {
                    Some(revert_state)
                } else {
                    match self.status {
                        AccountStatus::Destroyed |
                        AccountStatus::DestroyedAgain |
                        AccountStatus::LoadedNotExisting => {
                            // From destroyed to destroyed again is noop
                            //
                            // From DestroyedAgain to DestroyedAgain is noop
                            //
                            // From LoadedNotExisting to DestroyedAgain is noop
                            // as account is destroyed again
                            None
                        }
                        AccountStatus::DestroyedChanged => {
                            // From destroyed changed to destroyed again.
                            Some(AccountRevert::new_selfdestructed_again(
                                // Destroyed again will set empty account.
                                AccountStatus::DestroyedChanged,
                                AccountInfoRevert::RevertTo(self.info.clone().unwrap_or_default()),
                                core::mem::take(&mut self.storage),
                                HashMap::default(),
                            ))
                        }
                        _ => unreachable!("Invalid state to DestroyedAgain from {self:?}"),
                    }
                };
                // Set to destroyed and revert state.
                self.status = AccountStatus::DestroyedAgain;
                self.info = None;
                self.storage.clear();
                ret
            }
        };

        account_revert.filter(|acc| !acc.is_empty())
    }
}
