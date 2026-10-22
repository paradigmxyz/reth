//! Conversions between revm's bundle-state types, evm2's native types and Reth's account and
//! bytecode types.

use alloy_primitives::KECCAK256_EMPTY;
use reth_primitives_traits::Account;
use revm::state::{AccountInfo, Bytecode};

/// Converts a bundle-state account into Reth's account representation.
pub fn account_from_revm(info: &AccountInfo) -> Account {
    Account {
        balance: info.balance,
        nonce: info.nonce,
        bytecode_hash: (info.code_hash != KECCAK256_EMPTY).then_some(info.code_hash),
        #[cfg(feature = "account-ext")]
        extension: alloy_trie::AccountExtension::from_shared(info.extension.clone().into_shared()),
    }
}

/// Converts Reth's account into a bundle-state account without bytecode.
pub fn account_to_revm(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.bytecode_hash.unwrap_or(KECCAK256_EMPTY),
        code: None,
        account_id: None,
        #[cfg(feature = "account-ext")]
        extension: revm::state::AccountExtension::from_shared(account.extension.into_shared()),
    }
}

/// Converts bundle-state bytecode into Reth's bytecode wrapper.
pub fn reth_bytecode(code: &Bytecode) -> reth_primitives_traits::Bytecode {
    reth_primitives_traits::Bytecode(native_bytecode(code))
}

/// Converts persistent account information into evm2's execution representation.
pub fn native_account(info: &AccountInfo) -> evm2::evm::AccountInfo {
    evm2::evm::AccountInfo {
        #[cfg(feature = "account-ext")]
        extension: evm2::evm::AccountExtension::from_shared(info.extension.clone().into_shared()),
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: info.code.as_ref().map(native_bytecode),
        _non_exhaustive: (),
    }
}

/// Converts persistent bytecode while retaining its analyzed jump destinations and padding.
pub fn native_bytecode(code: &Bytecode) -> evm2::bytecode::Bytecode {
    if code.is_empty() {
        return evm2::bytecode::Bytecode::default()
    }
    if let Some(jumps) = code.legacy_jump_table() {
        let jumps = evm2::bytecode::JumpTable::from_slice(jumps.as_slice(), code.len());
        // SAFETY: revm's legacy padding completes any truncated trailing PUSH or
        // DUPN/SWAPN/EXCHANGE immediate and terminates the code with STOP, so decoding
        // immediates never reads past the buffer, which is what `new_analyzed` requires. evm2's
        // own `new_legacy` pads a fixed 33 bytes instead, so the buffers can differ in length.
        // The jump map comes from the same analyzed value.
        unsafe { evm2::bytecode::Bytecode::new_analyzed(code.bytes(), code.len(), jumps) }
    } else {
        evm2::bytecode::Bytecode::new_raw(code.original_bytes())
    }
}

/// Converts native bytecode into the persistent state representation.
pub fn revm_bytecode(code: &evm2::bytecode::Bytecode) -> Bytecode {
    if code.is_eip7702() {
        Bytecode::new_raw(code.original_bytes())
    } else {
        Bytecode::new_legacy(code.original_bytes())
    }
}

/// Converts native account information into the persistent state representation.
/// Bytecode is carried separately by the change stream; account updates only need its hash.
#[cfg_attr(not(feature = "account-ext"), allow(clippy::missing_const_for_fn))]
pub fn revm_account(info: &evm2::evm::AccountInfo) -> AccountInfo {
    AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: None,
        account_id: None,
        #[cfg(feature = "account-ext")]
        extension: revm::state::AccountExtension::from_shared(info.extension.clone().into_shared()),
    }
}
