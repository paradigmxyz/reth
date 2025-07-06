//! Contains the `BalanceChange` struct, which represents a post balance for an account.
//! Single balance change: `tx_index` -> `post_balance`

use alloy_primitives::TxIndex;

/// This struct is used to track the balance changes of accounts in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceChange {
    /// The index of the transaction that caused this balance change.
    pub tx_index: TxIndex,
    /// The post-transaction balance of the account.
    pub post_balance: u128,
}

impl BalanceChange {
    /// Creates a new `BalanceChange`.
    pub const fn new(tx_index: TxIndex, post_balance: u128) -> Self {
        Self { tx_index, post_balance }
    }

    /// Returns the transaction index.
    #[inline]
    pub const fn tx_index(&self) -> TxIndex {
        self.tx_index
    }

    /// Returns the post-transaction balance.
    #[inline]
    pub const fn post_balance(&self) -> u128 {
        self.post_balance
    }
}
