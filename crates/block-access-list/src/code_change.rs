//! Contains the `CodeChange` struct, which represents a new code for an account.
//! Single code change: `tx_index` -> `new_code`

use alloy_primitives::{Bytes, TxIndex};

use crate::MAX_CODE_SIZE;

/// This struct is used to track the new codes of accounts in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeChange {
    /// The index of the transaction that caused this balance change.
    pub tx_index: TxIndex,
    /// The new code of the account.
    pub new_code: Vec<Bytes>,
}
impl CodeChange {
    /// Creates a new `CodeChange`.
    pub fn new(tx_index: TxIndex) -> Self {
        Self { tx_index, new_code: Vec::with_capacity(MAX_CODE_SIZE) }
    }

    /// Returns the transaction index.
    #[inline]
    pub const fn tx_index(&self) -> TxIndex {
        self.tx_index
    }

    /// Returns the new code.
    #[inline]
    pub fn new_code(&self) -> &[Bytes] {
        &self.new_code
    }
}
