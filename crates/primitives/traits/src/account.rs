use alloy_primitives::{B256, U256};

/// Represents a minimal Account.
pub trait Account {
    /// Get `nonce`.
    fn nonce(&self) -> u64;

    /// Get `balance`.
    fn balance(&self) -> U256;

    /// Get `bytecode_hash`.
    fn bytecode_hash(&self) -> Option<B256>;
}

impl Account for revm_primitives::AccountInfo {
    fn nonce(&self) -> u64 {
        self.nonce
    }

    fn balance(&self) -> U256 {
        self.balance
    }

    fn bytecode_hash(&self) -> Option<B256> {
        Some(self.code_hash)
    }
}