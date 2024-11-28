use crate::states::{changes::ScrollPlainStateReverts, ScrollAccountInfo};
use reth_scroll_primitives::ScrollPostExecutionContext;
use revm::{
    db::{
        states::{reverts::AccountInfoRevert, PlainStorageRevert},
        AccountRevert, AccountStatus, RevertToSlot,
    },
    primitives::{map::HashMap, Address, U256},
};
use std::ops::{Deref, DerefMut};

/// Code copy of a [`revm::db::states::reverts::Reverts`] compatible with the
/// [`crate::states::ScrollBundleState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollReverts(Vec<Vec<(Address, ScrollAccountRevert)>>);

impl Deref for ScrollReverts {
    type Target = Vec<Vec<(Address, ScrollAccountRevert)>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ScrollReverts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl ScrollReverts {
    /// Create new reverts
    pub const fn new(reverts: Vec<Vec<(Address, ScrollAccountRevert)>>) -> Self {
        Self(reverts)
    }

    /// Extend reverts with other reverts.
    pub fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    /// Sort account inside transition by their address.
    pub fn sort(&mut self) {
        for revert in &mut self.0 {
            revert.sort_by_key(|(address, _)| *address);
        }
    }

    /// Generate a [`ScrollPlainStateReverts`].
    ///
    /// Note that account are sorted by address.
    pub fn to_plain_state_reverts(&self) -> ScrollPlainStateReverts {
        let mut state_reverts = ScrollPlainStateReverts::with_capacity(self.0.len());
        for reverts in &self.0 {
            // pessimistically pre-allocate assuming _all_ accounts changed.
            let mut accounts = Vec::with_capacity(reverts.len());
            let mut storage = Vec::with_capacity(reverts.len());
            for (address, revert_account) in reverts {
                match &revert_account.account {
                    ScrollAccountInfoRevert::RevertTo(acc) => {
                        // cloning is cheap, because account info has 3 small
                        // fields and a Bytes
                        accounts.push((*address, Some(acc.clone())))
                    }
                    ScrollAccountInfoRevert::DeleteIt => accounts.push((*address, None)),
                    ScrollAccountInfoRevert::DoNothing => (),
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
}

/// Code copy of a [`AccountRevert`] compatible with  [`ScrollReverts`].
#[derive(Clone, Default, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollAccountRevert {
    /// Account info revert
    pub account: ScrollAccountInfoRevert,
    /// Storage revert
    pub storage: HashMap<U256, RevertToSlot>,
    /// Previous status
    pub previous_status: AccountStatus,
    /// If true wipes storage
    pub wipe_storage: bool,
}

impl From<(AccountRevert, &ScrollPostExecutionContext)> for ScrollAccountRevert {
    fn from((account, context): (AccountRevert, &ScrollPostExecutionContext)) -> Self {
        Self {
            account: (account.account, context).into(),
            storage: account.storage,
            previous_status: account.previous_status,
            wipe_storage: account.wipe_storage,
        }
    }
}

impl ScrollAccountRevert {
    /// The approximate size of changes needed to store this account revert.
    /// `1 + storage_reverts_len`
    pub fn size_hint(&self) -> usize {
        1 + self.storage.len()
    }

    /// Returns `true` if there is nothing to revert,
    /// by checking that:
    /// * both account info and storage have been left untouched
    /// * we don't need to wipe storage
    pub fn is_empty(&self) -> bool {
        self.account == ScrollAccountInfoRevert::DoNothing &&
            self.storage.is_empty() &&
            !self.wipe_storage
    }
}

/// Code copy of a [`AccountInfoRevert`] compatible with the
/// [`ScrollAccountInfo`].
#[derive(Clone, Default, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ScrollAccountInfoRevert {
    #[default]
    /// Nothing changed
    DoNothing,
    /// Account was created and on revert we need to remove it with all storage.
    DeleteIt,
    /// Account was changed and on revert we need to put old state.
    RevertTo(ScrollAccountInfo),
}

impl From<(AccountInfoRevert, &ScrollPostExecutionContext)> for ScrollAccountInfoRevert {
    fn from((account, context): (AccountInfoRevert, &ScrollPostExecutionContext)) -> Self {
        match account {
            AccountInfoRevert::DoNothing => Self::DoNothing,
            AccountInfoRevert::DeleteIt => Self::DeleteIt,
            AccountInfoRevert::RevertTo(account) => Self::RevertTo((account, context).into()),
        }
    }
}
