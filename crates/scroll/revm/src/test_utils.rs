use crate::{
    shared::AccountInfo,
    states::{
        ScrollAccountInfoRevert, ScrollAccountRevert, ScrollPlainStateReverts, ScrollStateChangeset,
    },
    ScrollAccountInfo,
};
use reth_scroll_primitives::poseidon::{hash_code, POSEIDON_EMPTY};
use revm::db::{
    states::{reverts::AccountInfoRevert, PlainStateReverts, StateChangeset},
    AccountRevert,
};

// This conversion can cause a loss of information since performed without additional context.
impl From<StateChangeset> for ScrollStateChangeset {
    fn from(changeset: StateChangeset) -> Self {
        Self {
            accounts: changeset
                .accounts
                .into_iter()
                .map(|(add, acc)| (add, acc.map(Into::into)))
                .collect(),
            storage: changeset.storage,
            contracts: changeset.contracts,
        }
    }
}

// This conversion can cause a loss of information since performed without additional context.
impl From<PlainStateReverts> for ScrollPlainStateReverts {
    fn from(reverts: PlainStateReverts) -> Self {
        Self {
            accounts: reverts
                .accounts
                .into_iter()
                .map(|accounts| {
                    accounts.into_iter().map(|(add, acc)| (add, acc.map(Into::into))).collect()
                })
                .collect(),
            storage: reverts.storage,
        }
    }
}

// This conversion can cause a loss of information since performed without additional context.
impl From<AccountInfoRevert> for ScrollAccountInfoRevert {
    fn from(account: AccountInfoRevert) -> Self {
        match account {
            AccountInfoRevert::DoNothing => Self::DoNothing,
            AccountInfoRevert::DeleteIt => Self::DeleteIt,
            AccountInfoRevert::RevertTo(account) => Self::RevertTo(account.into()),
        }
    }
}

// This conversion can cause a loss of information since performed without additional context.
impl From<AccountRevert> for ScrollAccountRevert {
    fn from(account: AccountRevert) -> Self {
        Self {
            account: account.account.into(),
            storage: account.storage,
            previous_status: account.previous_status,
            wipe_storage: account.wipe_storage,
        }
    }
}

// This conversion can cause a loss of information since performed without additional context.
impl From<AccountInfo> for ScrollAccountInfo {
    fn from(info: AccountInfo) -> Self {
        let (code_size, poseidon_code_hash) = info
            .code
            .as_ref()
            .map(|code| (code.len() as u64, hash_code(code.original_byte_slice())))
            .unwrap_or((0, POSEIDON_EMPTY));
        Self {
            balance: info.balance,
            nonce: info.nonce,
            code_hash: info.code_hash,
            code: info.code,
            code_size,
            poseidon_code_hash,
        }
    }
}

// This conversion causes a loss of information.
impl From<ScrollAccountInfo> for AccountInfo {
    fn from(info: ScrollAccountInfo) -> Self {
        Self {
            balance: info.balance,
            nonce: info.nonce,
            code_hash: info.code_hash,
            code: info.code,
            #[cfg(feature = "scroll")]
            code_size: info.code_size as usize,
            #[cfg(feature = "scroll")]
            poseidon_code_hash: info.poseidon_code_hash,
        }
    }
}
