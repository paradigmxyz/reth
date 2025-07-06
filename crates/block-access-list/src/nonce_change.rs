//! Contains the `NonceChange` struct, which represents a new nonce for an account.
//! Single code change: `tx_index` -> `new_nonce`

use alloy_primitives::TxIndex;

/// This struct is used to track the new nonce of accounts in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonceChange {
    /// The index of the transaction that caused this nonce change.
    pub tx_index: TxIndex,
    /// The new code of the account.
    pub new_nonce: u64,
}

impl NonceChange {
    /// Creates a new `NonceChange`.
    pub const fn new(tx_index: TxIndex, new_nonce: u64) -> Self {
        Self { tx_index, new_nonce }
    }

    /// Returns the transaction index.
    pub const fn tx_index(&self) -> TxIndex {
        self.tx_index
    }

    /// Returns the new nonce.
    pub const fn new_nonce(&self) -> u64 {
        self.new_nonce
    }
}
