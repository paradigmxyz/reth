//! Contains the `BlockAccessList` struct, which represents a simple list of account changes.

use crate::{account_change::AccountChanges, MAX_ACCOUNTS};

/// This struct is used to store `account_changes` in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAccessList {
    /// List of account changes in the block.
    pub account_changes: Vec<AccountChanges>,
}

impl BlockAccessList {
    /// Creates a new `BlockAccessList` instance.
    pub fn new() -> Self {
        Self { account_changes: Vec::with_capacity(MAX_ACCOUNTS) }
    }

    /// Returns the account changes in the block.
    #[inline]
    pub fn account_changes(&self) -> &[AccountChanges] {
        &self.account_changes
    }
}
