//! Proof calculation version 2: Leaf-only implementation.
//!
//! This module provides a rewritten proof calculator that:
//! - Uses only leaf data (HashedAccounts/Storages) to generate proofs
//! - Returns proof nodes sorted lexicographically by path
//! - Automatically resets after each calculation
//! - Re-uses cursors across calculations
//! - Supports generic value types with lazy evaluation

use crate::{hashed_cursor::HashedCursor, trie_cursor::TrieCursor};
use alloy_primitives::B256;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::SparseTrieNode;

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
pub struct ProofCalculator<TC, HC, RF, VE: ValueEncoder<RlpNodeFut = RF>> {
    /// Trie cursor for traversing stored branch nodes.
    trie_cursor: TC,
    /// Hashed cursor for iterating over leaf data.
    hashed_cursor: HC,
    #[expect(unused)]
    /// Value encoder for converting values to RLP nodes.
    value_encoder: VE,
}

impl<TC, HC, RF, VE: ValueEncoder<RlpNodeFut = RF> + Default> ProofCalculator<TC, HC, RF, VE> {
    /// Create a new [`ProofCalculator`] instance.
    pub fn new(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self { trie_cursor, hashed_cursor, value_encoder: Default::default() }
    }
}

impl<TC, HC, RF, VE> ProofCalculator<TC, HC, RF, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    RF: RlpNodeFut,
    VE: ValueEncoder<RlpNodeFut = RF>,
{
    /// Replace the value encoder with a new one.
    ///
    /// This allows converting between different proof calculator types (e.g., to use a custom
    /// account value encoder) while reusing the same cursors.
    pub fn with_value_encoder<RF2, VE2>(
        self,
        value_encoder: VE2,
    ) -> ProofCalculator<TC, HC, RF2, VE2>
    where
        VE2: ValueEncoder<RlpNodeFut = RF2>,
        RF2: RlpNodeFut,
    {
        ProofCalculator {
            trie_cursor: self.trie_cursor,
            hashed_cursor: self.hashed_cursor,
            value_encoder,
        }
    }

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

/// A proof calculator for storage tries.
pub type StorageProofCalculator<TC, HC> =
    ProofCalculator<TC, HC, StorageRlpNodeFut, StorageValueEncoder>;
