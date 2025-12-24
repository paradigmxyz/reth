//! Thread-local sink for zombie accounts that need to be retained in the trie.
//!
//! This module provides a mechanism to pass zombie account addresses from the executor
//! to the trie layer. Zombie accounts are empty accounts that were deleted in a previous
//! transaction via EIP-161 cleanup and then resurrected in a subsequent transaction
//! via CreateZombieIfDeleted. These accounts must be preserved in the trie to match
//! Go nitro's behavior.
//!
//! The zombie mechanism is only active for ArbOS versions < 30 (pre-Stylus).

use alloy_primitives::{keccak256, Address, B256};
use alloc::collections::BTreeSet;
use core::cell::RefCell;

#[cfg(feature = "std")]
thread_local! {
    /// Set of zombie account addresses that should be retained in the trie.
    static ZOMBIE_ACCOUNTS: RefCell<BTreeSet<Address>> = RefCell::new(BTreeSet::new());
}

/// Clear all zombie accounts from the sink.
/// This should be called at the start of each block.
#[cfg(feature = "std")]
pub fn clear() {
    ZOMBIE_ACCOUNTS.with(|z| z.borrow_mut().clear());
}

/// Add a zombie account address to the sink.
/// This is called by CreateZombieIfDeleted when an account is resurrected.
#[cfg(feature = "std")]
pub fn push(address: Address) {
    ZOMBIE_ACCOUNTS.with(|z| {
        z.borrow_mut().insert(address);
    });
}

/// Take all zombie accounts from the sink and return them as hashed addresses.
/// This is called by the trie layer to get the set of accounts that should be retained.
#[cfg(feature = "std")]
pub fn take_hashed() -> BTreeSet<B256> {
    ZOMBIE_ACCOUNTS.with(|z| {
        let mut v = z.borrow_mut();
        let hashed: BTreeSet<B256> = v.iter().map(|addr| keccak256(addr)).collect();
        v.clear();
        hashed
    })
}

/// Check if there are any zombie accounts in the sink.
#[cfg(feature = "std")]
pub fn is_empty() -> bool {
    ZOMBIE_ACCOUNTS.with(|z| z.borrow().is_empty())
}

/// Get the current count of zombie accounts.
#[cfg(feature = "std")]
pub fn len() -> usize {
    ZOMBIE_ACCOUNTS.with(|z| z.borrow().len())
}

/// Peek at the zombie accounts without clearing them.
/// Returns a set of hashed addresses.
#[cfg(feature = "std")]
pub fn peek_hashed() -> BTreeSet<B256> {
    ZOMBIE_ACCOUNTS.with(|z| {
        z.borrow().iter().map(|addr| keccak256(addr)).collect()
    })
}

/// Add a hashed zombie account address to the sink.
/// This is used when we need to put back addresses after peeking.
#[cfg(feature = "std")]
pub fn push_hashed(hashed_address: B256) {
    // Note: We can't reverse the hash, so this is a no-op.
    // The zombie_sink should only be used via push() with raw addresses.
    let _ = hashed_address;
}

// No-op implementations for no_std
#[cfg(not(feature = "std"))]
pub fn clear() {}

#[cfg(not(feature = "std"))]
pub fn push(_address: Address) {}

#[cfg(not(feature = "std"))]
pub fn take_hashed() -> BTreeSet<B256> {
    BTreeSet::new()
}

#[cfg(not(feature = "std"))]
pub fn is_empty() -> bool {
    true
}

#[cfg(not(feature = "std"))]
pub fn len() -> usize {
    0
}

#[cfg(not(feature = "std"))]
pub fn peek_hashed() -> BTreeSet<B256> {
    BTreeSet::new()
}

#[cfg(not(feature = "std"))]
pub fn push_hashed(_hashed_address: B256) {}
