use crate::{revm_primitives::AccountInfo, Account, KECCAK_EMPTY};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Converts a Revm [`AccountInfo`] into a Reth [`Account`].
///
/// Sets `bytecode_hash` to `None` if `code_hash` is [`KECCAK_EMPTY`].
pub fn into_reth_acc(revm_acc: AccountInfo) -> Account {
    let code_hash = revm_acc.code_hash;
    Account {
        balance: revm_acc.balance,
        nonce: revm_acc.nonce,
        bytecode_hash: (code_hash != KECCAK_EMPTY).then_some(code_hash),
    }
}

/// Converts a Revm [`AccountInfo`] into a Reth [`Account`].
///
/// Sets `code_hash` to [`KECCAK_EMPTY`] if `bytecode_hash` is `None`.
pub fn into_revm_acc(reth_acc: Account) -> AccountInfo {
    AccountInfo {
        balance: reth_acc.balance,
        nonce: reth_acc.nonce,
        code_hash: reth_acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        code: None,
    }
}
