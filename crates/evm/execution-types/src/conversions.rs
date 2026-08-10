use alloy_consensus::constants::KECCAK_EMPTY;
use reth_primitives_traits::{Account, Bytecode};

pub(crate) fn account_from_revm(info: &revm::state::AccountInfo) -> Account {
    Account {
        nonce: info.nonce,
        balance: info.balance,
        bytecode_hash: (info.code_hash != KECCAK_EMPTY).then_some(info.code_hash),
    }
}

pub(crate) fn account_into_revm(account: Account) -> revm::state::AccountInfo {
    revm::state::AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        code: None,
        account_id: None,
    }
}

pub(crate) fn bytecode_from_revm(bytecode: revm::bytecode::Bytecode) -> Bytecode {
    Bytecode::new_raw(bytecode.original_bytes())
}

pub(crate) fn bytecode_into_revm(bytecode: Bytecode) -> revm::bytecode::Bytecode {
    revm::bytecode::Bytecode::new_raw(bytecode.0.original_bytes())
}
