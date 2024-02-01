use crate::{
    revm_primitives::{AccountInfo, Log},
    Account, Address, Log as RethLog, TransactionKind, KECCAK_EMPTY, U256,
};
use revm::{
    interpreter::gas::validate_initial_tx_gas,
    primitives::{MergeSpec, ShanghaiSpec},
};

/// Check equality between Revm and Reth `Log`s.
pub fn is_log_equal(revm_log: &Log, reth_log: &RethLog) -> bool {
    revm_log.address == reth_log.address &&
        revm_log.data.data == reth_log.data &&
        revm_log.topics() == reth_log.topics
}

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

/// Calculates the Intrinsic Gas usage for a Transaction
///
/// Caution: This only checks past the Merge hardfork.
#[inline]
pub fn calculate_intrinsic_gas_after_merge(
    input: &[u8],
    kind: &TransactionKind,
    access_list: &[(Address, Vec<U256>)],
    is_shanghai: bool,
) -> u64 {
    if is_shanghai {
        validate_initial_tx_gas::<ShanghaiSpec>(input, kind.is_create(), access_list)
    } else {
        validate_initial_tx_gas::<MergeSpec>(input, kind.is_create(), access_list)
    }
}
