use reth_primitives::{
    revm_primitives::{AccountInfo, Log},
    Account, Log as RethLog, KECCAK_EMPTY,
};

/// Check equality between [`reth_primitives::Log`] and [`revm::primitives::Log`]
pub fn is_log_equal(revm_log: &Log, reth_log: &reth_primitives::Log) -> bool {
    revm_log.address == reth_log.address &&
        revm_log.data == reth_log.data &&
        revm_log.topics == reth_log.topics
}

/// Into reth primitive [Log] from [revm::primitives::Log].
pub fn into_reth_log(log: Log) -> RethLog {
    RethLog { address: log.address, topics: log.topics, data: log.data }
}

/// Create reth primitive [Account] from [revm::primitives::AccountInfo].
/// Check if revm bytecode hash is [KECCAK_EMPTY] and put None to reth [Account]
pub fn into_reth_acc(revm_acc: AccountInfo) -> Account {
    let code_hash = revm_acc.code_hash;
    Account {
        balance: revm_acc.balance,
        nonce: revm_acc.nonce,
        bytecode_hash: (code_hash != KECCAK_EMPTY).then_some(code_hash),
    }
}

/// Create revm primitive [AccountInfo] from [reth_primitives::Account].
pub fn into_revm_acc(reth_acc: Account) -> AccountInfo {
    AccountInfo {
        balance: reth_acc.balance,
        nonce: reth_acc.nonce,
        code_hash: reth_acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        code: None,
    }
}
