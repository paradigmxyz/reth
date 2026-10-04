//! Partial advisory proof targets derived without executing a transaction or reading state.

use alloy_primitives::{Address, U256};

/// A best-effort proof key. It carries no value or execution/validation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProofKeyHint {
    /// Request the account proof for this address.
    Account(Address),
    /// Request the storage proof for this address and unhashed slot.
    Storage(Address, U256),
}
