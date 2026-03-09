//! Generic `SparseTrie` test suite.
//!
//! Tests are written as generic functions `test_foo<T: SparseTrie + Default>()` and stamped out
//! for every concrete implementation via the [`sparse_trie_tests`] macro.
//!
//! Tests are organized into modules by which `SparseTrie` method is the most likely root cause
//! of failure for each test case:
//!
//! - [`set_root`]: Tests for `set_root` / `with_root`
//! - [`reveal_nodes`]: Tests for `reveal_nodes` / `reveal_node`
//! - [`update_leaves`]: Tests for `update_leaves`, including insert, modify, and remove
//! - [`root`]: Tests for `root()` hash computation
//! - [`take_updates`]: Tests for `take_updates`
//! - [`commit_updates`]: Tests for `commit_updates`
//! - [`prune`]: Tests for `prune`
//! - [`wipe_clear`]: Tests for `wipe` and `clear`
//! - [`get_leaf_value`]: Tests for `get_leaf_value`
//! - [`find_leaf`]: Tests for `find_leaf`
//! - [`size_hint`]: Tests for `size_hint`
//! - [`lifecycle`]: Integration tests exercising multiple methods together

use alloy_primitives::{map::B256Map, B256, U256};
use alloy_rlp::{encode_fixed_size, Decodable};
use alloy_trie::EMPTY_ROOT_HASH;
use reth_trie::{
    hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
    prefix_set::PrefixSet,
    proof_v2::StorageProofCalculator,
    trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
    StorageRoot,
};
use reth_trie_common::{
    updates::StorageTrieUpdates, Nibbles, ProofTrieNodeV2, ProofV2Target, TrieNodeV2,
};
use reth_trie_sparse::{LeafLookup, LeafLookupError, LeafUpdate, SparseTrie};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    iter::once,
};

mod commit_updates;
mod find_leaf;
mod get_leaf_value;
mod lifecycle;
mod prune;
mod reveal_nodes;
mod root;
mod set_root;
mod size_hint;
mod take_updates;
mod update_leaves;
mod wipe_clear;

/// A fixed hashed address used by the harness for all storage trie operations.
const HASHED_ADDRESS: B256 = B256::ZERO;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Generic test harness for `SparseTrie` tests.
///
/// Manages a base storage dataset, computes expected roots via `StorageRoot`, and generates
/// V2 proofs via `StorageProofCalculator` using mock cursors.
struct SuiteTestHarness {
    /// The base storage dataset (hashed slot → value). Zero-valued entries are absent.
    storage: BTreeMap<B256, U256>,
    /// The expected storage root, calculated by `StorageRoot`.
    original_root: B256,
    /// The starting storage trie updates, used for minimization.
    storage_trie_updates: StorageTrieUpdates,
    /// Mock factory for trie cursors.
    trie_cursor_factory: MockTrieCursorFactory,
    /// Mock factory for hashed cursors.
    hashed_cursor_factory: MockHashedCursorFactory,
}

impl SuiteTestHarness {
    /// Creates a new test harness from a map of hashed storage slots to values.
    fn new(storage: BTreeMap<B256, U256>) -> Self {
        let mut harness = Self {
            storage: BTreeMap::new(),
            original_root: B256::ZERO,
            storage_trie_updates: StorageTrieUpdates::default(),
            trie_cursor_factory: MockTrieCursorFactory::new(BTreeMap::new(), Default::default()),
            hashed_cursor_factory: MockHashedCursorFactory::new(
                BTreeMap::new(),
                Default::default(),
            ),
        };
        harness.apply_changeset(storage);
        harness
    }

    /// Merges `changeset` into the base storage (zero values remove entries) and
    /// recomputes the storage root, trie updates, and cursor factories.
    fn apply_changeset(&mut self, changeset: BTreeMap<B256, U256>) {
        for (k, v) in changeset {
            if v == U256::ZERO {
                self.storage.remove(&k);
            } else {
                self.storage.insert(k, v);
            }
        }

        self.hashed_cursor_factory = MockHashedCursorFactory::new(
            BTreeMap::new(),
            once((HASHED_ADDRESS, self.storage.clone())).collect(),
        );

        let empty_trie_cursor_factory = MockTrieCursorFactory::new(
            BTreeMap::new(),
            once((HASHED_ADDRESS, BTreeMap::new())).collect(),
        );

        let (original_root, _, storage_trie_updates) = StorageRoot::new_hashed(
            empty_trie_cursor_factory,
            self.hashed_cursor_factory.clone(),
            HASHED_ADDRESS,
            PrefixSet::default(),
            #[cfg(feature = "metrics")]
            reth_trie::metrics::TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
        .root_with_updates()
        .expect("StorageRoot should succeed");

        self.trie_cursor_factory = MockTrieCursorFactory::new(
            BTreeMap::new(),
            once((
                HASHED_ADDRESS,
                storage_trie_updates.storage_nodes.iter().map(|(k, v)| (*k, v.clone())).collect(),
            ))
            .collect(),
        );

        self.original_root = original_root;
        self.storage_trie_updates = storage_trie_updates;
    }

    /// Obtains the root node of the storage trie via `StorageProofCalculator`.
    fn root_node(&self) -> ProofTrieNodeV2 {
        let trie_cursor = self
            .trie_cursor_factory
            .storage_trie_cursor(HASHED_ADDRESS)
            .expect("storage trie cursor should succeed");
        let hashed_cursor = self
            .hashed_cursor_factory
            .hashed_storage_cursor(HASHED_ADDRESS)
            .expect("hashed storage cursor should succeed");

        let mut proof_calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);
        proof_calculator
            .storage_root_node(HASHED_ADDRESS)
            .expect("storage_root_node should succeed")
    }

    /// Generates storage proofs for the given targets using `StorageProofCalculator`.
    fn proof_v2(&self, targets: &mut [ProofV2Target]) -> Vec<ProofTrieNodeV2> {
        let trie_cursor = self
            .trie_cursor_factory
            .storage_trie_cursor(HASHED_ADDRESS)
            .expect("storage trie cursor should succeed");
        let hashed_cursor = self
            .hashed_cursor_factory
            .hashed_storage_cursor(HASHED_ADDRESS)
            .expect("hashed storage cursor should succeed");

        let mut proof_calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);
        proof_calculator.storage_proof(HASHED_ADDRESS, targets).expect("proof_v2 should succeed")
    }

    /// Builds leaf updates from a changeset. Non-zero values become inserts/modifies,
    /// zero values become removals (empty vec).
    fn leaf_updates(changes: &BTreeMap<B256, U256>) -> B256Map<LeafUpdate> {
        changes
            .iter()
            .map(|(&slot, &value)| {
                let rlp_value = if value == U256::ZERO {
                    Vec::new()
                } else {
                    encode_fixed_size(&value).to_vec()
                };
                (slot, LeafUpdate::Changed(rlp_value))
            })
            .collect()
    }

    /// Runs the reveal-update loop on the given trie: repeatedly calls `update_leaves`,
    /// collects proof targets from the callback, fetches proofs, and reveals them until
    /// no more proofs are needed.
    fn reveal_and_update<T: SparseTrie>(
        &self,
        trie: &mut T,
        leaf_updates: &mut B256Map<LeafUpdate>,
    ) {
        loop {
            let mut targets: Vec<ProofV2Target> = Vec::new();
            trie.update_leaves(leaf_updates, |key, min_len| {
                targets.push(ProofV2Target::new(key).with_min_len(min_len));
            })
            .expect("update_leaves should succeed");

            if targets.is_empty() {
                break;
            }

            let mut proof_nodes = self.proof_v2(&mut targets);
            trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");
        }
    }

    /// Initializes a trie with the harness root node and reveals all proof nodes for the
    /// given target keys. Returns the initialized trie.
    fn init_trie_with_targets<T: SparseTrie + Default>(
        &self,
        target_keys: &[B256],
        retain_updates: bool,
    ) -> T {
        let root_node = self.root_node();
        let mut trie = T::default();
        trie.set_root(root_node.node, root_node.masks, retain_updates)
            .expect("set_root should succeed");

        if !target_keys.is_empty() {
            let mut targets: Vec<ProofV2Target> =
                target_keys.iter().map(|k| ProofV2Target::new(*k)).collect();
            let mut proof_nodes = self.proof_v2(&mut targets);
            trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");
        }

        trie
    }

    /// Initializes a trie and reveals proofs for all keys in the base storage.
    fn init_trie_fully_revealed<T: SparseTrie + Default>(&self, retain_updates: bool) -> T {
        let keys: Vec<B256> = self.storage.keys().copied().collect();
        self.init_trie_with_targets(&keys, retain_updates)
    }
}

// ---------------------------------------------------------------------------
// Macro: stamp out tests for every SparseTrie impl
// ---------------------------------------------------------------------------

/// Stamps out `#[test]` functions for each generic test function listed, instantiated
/// for every concrete `SparseTrie` implementation.
macro_rules! sparse_trie_tests {
    ( $( $test_fn:ident ),* $(,)? ) => {
        mod parallel_sparse_trie {
            use reth_trie_sparse::ParallelSparseTrie;

            $(
                #[test]
                fn $test_fn() {
                    super::$test_fn::<ParallelSparseTrie>();
                }
            )*
        }

    };
}

// ---------------------------------------------------------------------------
// Re-export test functions from submodules for the macro
// ---------------------------------------------------------------------------

use commit_updates::*;
use find_leaf::*;
use get_leaf_value::*;
use lifecycle::*;
use prune::*;
use reveal_nodes::*;
use root::*;
use set_root::*;
use size_hint::*;
use take_updates::*;
use update_leaves::*;
use wipe_clear::*;

// ---------------------------------------------------------------------------
// Test registration
// ---------------------------------------------------------------------------

sparse_trie_tests! {
    // set_root
    test_set_root_with_branch_node,
    test_set_root_with_leaf_node,
    test_set_root_with_extension_node,
    test_set_root_retains_updates_when_requested,
    test_set_root_does_not_retain_updates_when_not_requested,
    test_set_root_with_branch_masks,
    test_set_root_with_empty_root,

    // reveal_nodes
    test_reveal_nodes_empty_slice,
    test_reveal_nodes_single_leaf,
    test_reveal_nodes_idempotent,
    test_reveal_nodes_with_branch_masks,
    test_reveal_nodes_skips_on_empty_root,
    test_reveal_nodes_filters_unreachable_boundary_leaves,
    test_reveal_insert_reveal_preserves_branch_state,
    test_remove_then_reveal_does_not_overwrite_collapsed_node,
    test_insert_then_reveal_does_not_overwrite_branch,

    // update_leaves
    test_update_leaves_insert_new_leaf,
    test_update_leaves_modify_existing_leaf,
    test_insert_single_leaf_into_empty_trie,
    test_insert_multiple_leaves_into_empty_trie,
    test_update_all_leaves_with_new_values,
    test_two_leaves_at_adjacent_keys_root_correctness,
    test_update_leaves_remove_leaf,
    test_remove_leaf_branch_collapses_to_extension,
    test_remove_leaf_branch_collapses_to_leaf,
    test_remove_last_leaf_produces_empty_root,
    test_insert_then_remove_sequence,
    test_remove_nonexistent_leaf_preserves_hashes,
    test_update_leaves_blinded_node_requests_proof,
    test_update_leaves_retry_after_reveal,
    test_remove_leaf_blinded_sibling_requires_reveal,
    test_update_leaves_removal_branch_collapse_blinded_sibling,
    test_update_leaves_subtrie_collapse_requests_proof,
    test_update_leaves_multiple_keys_same_blinded_node,
    test_update_leaves_touched_fully_revealed,
    test_update_leaves_touched_blinded_requests_proof,
    test_update_leaves_touched_nonexistent_key,
    test_update_leaves_touched_nonexistent_in_populated_trie,
    test_update_leaves_multiple_mixed_updates,
    test_remove_leaf_marks_ancestors_dirty_unconditionally,
    test_orphaned_value_update_falls_through_to_full_insertion,
    test_branch_collapse_updates_leaf_key_len_across_subtries,
    test_remove_leaf_does_not_reveal_blind_subtries,

    // root
    test_root_empty_trie,
    test_root_cached_returns_without_recomputation,
    test_root_after_single_leaf_update,
    test_root_deterministic_across_update_orders,
    test_root_handles_small_root_node_without_hash,

    // take_updates
    test_take_updates_returns_empty_when_not_tracking,
    test_take_updates_resets_after_take,
    test_take_updates_contains_updated_and_removed_nodes,
    test_take_updates_no_duplicate_updated_and_removed_nodes,

    // commit_updates
    test_commit_updates_syncs_branch_masks,
    test_commit_updates_empty_is_noop,

    // prune
    test_prune_retains_specified_leaves,
    test_prune_reduces_node_count,
    test_prune_empty_retained_set,
    test_prune_requires_computed_hashes,
    test_prune_then_update_and_recompute_root,
    test_prune_then_reveal_pruned_subtree,
    test_prune_mixed_embedded_and_hashed_nodes,
    test_prune_then_update_no_panic,
    test_prune_only_descends_into_branch_root,
    test_prune_handles_small_subtrie_root_nodes,

    // wipe / clear
    test_wipe_resets_to_empty_root,
    test_clear_resets_trie_but_preserves_update_tracking,
    test_wipe_produces_wiped_updates,
    test_clear_then_reuse_trie,

    // get_leaf_value
    test_get_leaf_value_after_update,
    test_get_leaf_value_after_removal,

    // find_leaf
    test_find_leaf_exists,
    test_find_leaf_nonexistent,
    test_find_leaf_blinded,
    test_find_leaf_value_mismatch,
    test_find_leaf_nonexistent_branch_divergence,
    test_find_leaf_nonexistent_extension_divergence,
    test_find_leaf_nonexistent_leaf_divergence,

    // size_hint
    test_size_hint_reflects_leaf_count,

    // lifecycle (integration tests)
    test_full_lifecycle_update_root_take_commit,
    test_multi_round_update_commit_prune_cycle,
    test_reveal_update_root_basic_lifecycle,
    test_incremental_reveal_and_update_with_retry,
    test_full_block_processing_lifecycle,
    test_touched_prewarm_then_changed_update,
    test_touched_on_blinded_triggers_proof_then_changed_succeeds,
    test_get_leaf_value_for_storage_root_lookup,
    test_find_leaf_before_update_to_check_existence,
    test_prune_then_reuse_for_next_block,
}
