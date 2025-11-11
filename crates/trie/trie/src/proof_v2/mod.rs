//! Proof calculation version 2: Leaf-only implementation.
//!
//! This module provides a rewritten proof calculator that:
//! - Uses only leaf data (HashedAccounts/Storages) to generate proofs
//! - Returns proof nodes sorted lexicographically by path
//! - Automatically resets after each calculation
//! - Re-uses cursors across calculations
//! - Supports generic value types with lazy evaluation

use crate::{
    hashed_cursor::{HashedCursor, HashedStorageCursor},
    trie_cursor::{TrieCursor, TrieStorageCursor},
};
use alloy_primitives::{B256, U256};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{Nibbles, SparseTrieNode, TrieMasks, TrieNode};

mod targets;
pub use targets::*;

mod value;
pub use value::*;

/// A proof calculator that generates merkle proofs using only leaf data.
///
/// The calculator:
/// - Accepts one or more B256 proof targets sorted lexicographically
/// - Returns proof nodes sorted lexicographically by path
/// - Returns only the root when given zero targets
/// - Automatically resets after each calculation
/// - Re-uses cursors from one calculation to the next
#[derive(Debug)]
pub struct ProofCalculator<TC, HC, VE> {
    /// Trie cursor for traversing stored branch nodes.
    trie_cursor: TC,
    /// Hashed cursor for iterating over leaf data.
    hashed_cursor: HC,
    value_encoder: core::marker::PhantomData<VE>,
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE> {
    /// Create a new [`ProofCalculator`] instance for calculating account proofs.
    pub const fn new(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self { trie_cursor, hashed_cursor, value_encoder: core::marker::PhantomData }
    }
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: ValueEncoder<Value = HC::Value>,
{
    /// Internal implementation of proof calculation. Assumes both cursors have already been reset.
    /// See docs on [`Self::proof`] for expected behavior.
    fn proof_inner(
        &self,
        _value_encoder: &VE,
        targets: impl IntoIterator<Item = B256>,
    ) -> Result<Vec<SparseTrieNode>, StateProofError> {
        // In debug builds, verify that targets are sorted
        #[cfg(debug_assertions)]
        let targets = {
            let mut prev: Option<B256> = None;
            targets.into_iter().inspect(move |target| {
                if let Some(prev) = prev {
                    debug_assert!(
                        prev <= *target,
                        "targets must be sorted lexicographically: {:?} > {:?}",
                        prev,
                        target
                    );
                }
                prev = Some(*target);
            })
        };

        #[cfg(not(debug_assertions))]
        let targets = targets.into_iter();

        // Silence unused variable warning for now
        let _ = targets;

        todo!("proof not yet implemented")
    }
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: ValueEncoder<Value = HC::Value>,
{
    /// Generate a proof for the given targets.
    ///
    /// Given target keys sorted lexicographically, returns proof nodes
    /// for all targets sorted lexicographically by path.
    ///
    /// If given zero targets, returns just the root.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    pub fn proof(
        &mut self,
        value_encoder: &VE,
        targets: impl IntoIterator<Item = B256>,
    ) -> Result<Vec<SparseTrieNode>, StateProofError> {
        self.trie_cursor.reset();
        self.hashed_cursor.reset();
        self.proof_inner(value_encoder, targets)
    }
}

/// A proof calculator for storage tries.
pub type StorageProofCalculator<TC, HC> = ProofCalculator<TC, HC, StorageValueEncoder>;

/// Static storage value encoder instance used by all storage proofs.
static STORAGE_VALUE_ENCODER: StorageValueEncoder = StorageValueEncoder;

impl<TC, HC> StorageProofCalculator<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = U256>,
{
    /// Create a new [`StorageProofCalculator`] instance.
    pub const fn new_storage(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self::new(trie_cursor, hashed_cursor)
    }

    /// Generate a proof for a storage trie at the given hashed address.
    ///
    /// Given target keys sorted lexicographically, returns proof nodes
    /// for all targets sorted lexicographically by path.
    ///
    /// If given zero targets, returns just the root.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    pub fn storage_proof(
        &mut self,
        hashed_address: B256,
        targets: impl IntoIterator<Item = B256>,
    ) -> Result<Vec<SparseTrieNode>, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        // Shortcut: check if storage is empty
        if self.hashed_cursor.is_storage_empty()? {
            // Return a single EmptyRoot node at the root path
            return Ok(vec![SparseTrieNode {
                path: Nibbles::default(),
                node: TrieNode::EmptyRoot,
                masks: TrieMasks::none(),
            }])
        }

        // Don't call `set_hashed_address` on the trie cursor until after the previous shortcut has
        // been checked.
        self.trie_cursor.set_hashed_address(hashed_address);

        // Use the static StorageValueEncoder and pass it to proof_inner
        self.proof_inner(&STORAGE_VALUE_ENCODER, targets)
    }
}
