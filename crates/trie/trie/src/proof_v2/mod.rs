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
use alloy_trie::TrieMask;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{BranchNode, Nibbles, ProofTrieNode, RlpNode, TrieMasks, TrieNode};
use tracing::{instrument, trace};

mod value;
pub use value::*;

mod node;
use node::*;

/// Target to use with the `tracing` crate.
static TRACE_TARGET: &str = "trie::proof_v2";

/// A proof calculator that generates merkle proofs using only leaf data.
///
/// The calculator:
/// - Accepts one or more B256 proof targets sorted lexicographically
/// - Returns proof nodes sorted lexicographically by path
/// - Returns only the root when given zero targets
/// - Automatically resets after each calculation
/// - Re-uses cursors from one calculation to the next
#[derive(Debug)]
pub struct ProofCalculator<TC, HC, VE: LeafValueEncoder> {
    /// Trie cursor for traversing stored branch nodes.
    trie_cursor: TC,
    /// Hashed cursor for iterating over leaf data.
    hashed_cursor: HC,
    /// Branches which are currently in the process of being constructed, each being a child of
    /// the previous one.
    branch_stack: Vec<ProofTrieBranch>,
    /// The path of the last branch in `branch_stack`.
    branch_path: Nibbles,
    /// Children of branches in the `branch_stack`.
    ///
    /// Each branch in `branch_stack` tracks which children are in this stack using its
    /// `state_mask`; the number of children the branch has in this stack is equal to the number of
    /// bits set in its `state_mask`.
    ///
    /// The children for the bottom branch in `branch_stack` are found at the bottom of this stack,
    /// and so on. When a branch is removed from `branch_stack` its children are removed from this
    /// one, and the branch's [`RlpNode`] is pushed onto this stack in their place (see
    /// [`Self::pop_branch`].
    child_stack: Vec<ProofTrieBranchChild<VE::DeferredEncoder>>,
    /// Free-list of re-usable buffers of [`RlpNode`]s, used for encoding branch nodes to RLP.
    ///
    /// We are generally able to re-use these buffers across different branch nodes for the
    /// duration of a proof calculation, but occasionally we will lose one when when a branch
    /// node is returned as a `ProofTrieNode`.
    rlp_nodes_bufs: Vec<Vec<RlpNode>>,
    /// Re-usable byte buffer, used for RLP encoding.
    rlp_encode_buf: Vec<u8>,
}

impl<TC, HC, VE: LeafValueEncoder> ProofCalculator<TC, HC, VE> {
    /// Create a new [`ProofCalculator`] instance for calculating account proofs.
    pub const fn new(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self {
            trie_cursor,
            hashed_cursor,
            branch_stack: Vec::<_>::new(),
            branch_path: Nibbles::new(),
            child_stack: Vec::<_>::new(),
            rlp_nodes_bufs: Vec::<_>::new(),
            rlp_encode_buf: Vec::<_>::new(),
        }
    }
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: LeafValueEncoder<Value = HC::Value>,
{
    /// Takes a re-usable `RlpNode` buffer from the internal free-list, or allocates a new one if
    /// the free-list is empty.
    ///
    /// The returned Vec will have a length of zero.
    fn take_rlp_nodes_buf(&mut self) -> Vec<RlpNode> {
        self.rlp_nodes_bufs
            .pop()
            .map(|mut buf| {
                buf.clear();
                buf
            })
            .unwrap_or_else(|| Vec::with_capacity(16))
    }

    /// Pushes a new branch onto the `branch_stack`, while also pushing the given leaf onto the
    /// `child_stack`.
    ///
    /// This method expects that there already exists a child on the `child_stack`, and that that
    /// child has a non-zero short key. The new branch is constructed based on the top child from
    /// the `child_stack` and the given leaf.
    fn push_new_branch(&mut self, leaf_key: Nibbles, leaf_val: VE::DeferredEncoder) {
        // First determine the new leaf's shortkey relative to the current branch. If there is no
        // current branch then the short key is the full key.
        let leaf_short_key = if self.branch_stack.is_empty() {
            leaf_key
        } else {
            // When there is a current branch then trim off its path as well as the nibble that it
            // has set for this leaf.
            leaf_key.slice_unchecked(self.branch_path.len() + 1, leaf_key.len())
        };

        trace!(
            target: TRACE_TARGET,
            ?leaf_short_key,
            branch_path = ?self.branch_path,
            "push_new_branch: called",
        );

        // Get the new branch's first child, which is the child on the top of the stack with which
        // the new leaf shares the same nibble on the current branch.
        let first_child = self
            .child_stack
            .last_mut()
            .expect("push_branch can't be called with empty child_stack");

        let first_child_short_key = first_child.short_key();
        debug_assert!(
            !first_child_short_key.is_empty(),
            "push_branch called when top child on stack is not a leaf or extension with a short key",
        );

        // Determine how many nibbles are shared between the new branch's first child and the new
        // leaf. This common prefix will be the extension of the new branch
        let common_prefix_len = first_child_short_key.common_prefix_length(&leaf_short_key);

        // Trim off the common prefix from the first child's short key, plus one nibble which will
        // stored by the new branch itself in its state mask.
        let first_child_nibble = first_child_short_key.get_unchecked(common_prefix_len);
        first_child.trim_short_key_prefix(common_prefix_len + 1);

        // Similarly, trim off the common prefix, plus one nibble for the new branch, from the new
        // leaf's short key.
        let leaf_nibble = leaf_short_key.get_unchecked(common_prefix_len);
        let leaf_short_key = trim_nibbles_prefix(&leaf_short_key, common_prefix_len + 1);

        // Push the new leaf onto the child stack; it will be the second child of the new branch.
        // The new branch's first child is the child already on the top of the stack, for which
        // we've already adjusted its short key.
        self.child_stack
            .push(ProofTrieBranchChild::Leaf { short_key: leaf_short_key, value: leaf_val });

        // Construct the state mask of the new branch, and push the new branch onto the branch
        // stack.
        self.branch_stack.push(ProofTrieBranch {
            ext_len: common_prefix_len as u8,
            state_mask: {
                let mut m = TrieMask::default();
                m.set_bit(first_child_nibble);
                m.set_bit(leaf_nibble);
                m
            },
            tree_mask: TrieMask::default(),
            hash_mask: TrieMask::default(),
        });

        // Update the branch path to reflect the new branch which was just pushed. Its path will be
        // the path of the previous branch, plus the nibble shared by each child, plus the parent
        // extension (denoted by a non-zero `ext_len`). Since the new branch's path is a prefix of
        // the original leaf_key we can just slice that.
        //
        // If the branch is the first branch then we do not add the extra 1, as there is no nibble
        // in a parent branch to account for.
        let branch_path_len = self.branch_path.len() +
            common_prefix_len +
            if self.branch_stack.len() == 1 { 0 } else { 1 };
        self.branch_path = leaf_key.slice_unchecked(0, branch_path_len);

        trace!(
            target: TRACE_TARGET,
            ?leaf_short_key,
            ?common_prefix_len,
            new_branch = ?self.branch_stack.last().unwrap(),
            ?branch_path_len,
            branch_path = ?self.branch_path,
            "push_new_branch: returning",
        );
    }

    /// Pops the top branch off of the `branch_stack`, and hashes it (and its extension node as
    /// well, if there is one). The `branch_path` field will be updated accordingly.
    ///
    /// If there remains another branch on the stack then that branch is updated. If not then a
    /// single [`ProofTrieBranchChild::Hash`] is left on the `child_stack`.
    ///
    /// # Panics
    ///
    /// This method panics if `branch_stack` is empty.
    fn pop_branch(&mut self) -> Result<(), StateProofError> {
        let mut rlp_nodes_buf = self.take_rlp_nodes_buf();
        let branch = self.branch_stack.pop().expect("branch_stack cannot be empty");

        trace!(
            target: TRACE_TARGET,
            ?branch,
            branch_path = ?self.branch_path,
            "pop_branch: called",
        );

        // Take the branch's children off the stack, using the state mask to determine how many
        // there are.
        let num_children = branch.state_mask.count_ones() as usize;
        debug_assert!(num_children > 1, "A branch must have at least two children");
        debug_assert!(
            self.child_stack.len() >= num_children,
            "Stack is missing necessary children"
        );
        let children = self.child_stack.drain(self.child_stack.len() - num_children..);

        // We will be pushing the branch onto the child stack, which will require its parent
        // extension's short key (if it has a parent extension). Calculate this short key from the
        // `branch_path` prior to modifying the `branch_path`.
        let short_key = trim_nibbles_prefix(
            &self.branch_path,
            self.branch_path.len() - branch.ext_len as usize,
        );

        // Update the branch_path. If this branch is the only branch then only its extension needs
        // to be trimmed, otherwise we also need to remove its nibble from its parent.
        let new_path_len = self.branch_path.len() -
            branch.ext_len as usize -
            if self.branch_stack.is_empty() { 0 } else { 1 };

        debug_assert!(self.branch_path.len() >= new_path_len);
        self.branch_path = self.branch_path.slice_unchecked(0, new_path_len);

        // From here we will be encoding the branch node and pushing it onto the child stack,
        // replacing its children.

        // Collect children into an `RlpNode` Vec by calling into_rlp on each.
        for child in children {
            let (child_rlp_node, freed_rlp_nodes_buf) = child.into_rlp(&mut self.rlp_encode_buf)?;
            rlp_nodes_buf.push(child_rlp_node);

            // If there is an `RlpNode` buffer which can be re-used then push it onto the free-list.
            if let Some(buf) = freed_rlp_nodes_buf {
                self.rlp_nodes_bufs.push(buf);
            }
        }

        debug_assert_eq!(
            rlp_nodes_buf.len(),
            branch.state_mask.count_ones() as usize,
            "children length must match number of bits set in state_mask"
        );

        // Construct the `BranchNode`.
        let branch_node = BranchNode::new(rlp_nodes_buf, branch.state_mask);

        // Wrap the `BranchNode` so it can be pushed onto the child stack.
        let branch_as_child = if short_key.is_empty() {
            // If there is no extension then push a branch node
            ProofTrieBranchChild::Branch(branch_node)
        } else {
            // Otherwise push an extension node
            ProofTrieBranchChild::Extension { short_key, child: branch_node }
        };

        self.child_stack.push(branch_as_child);
        Ok(())
    }

    /// Adds a single leaf for a key to the stack, possibly collapsing an existing branch and/or
    /// creating a new one depending on the path of the key.
    fn add_leaf(&mut self, key: Nibbles, val: VE::DeferredEncoder) -> Result<(), StateProofError> {
        loop {
            // Get the branch currently being built. If there are no branches on the stack then it
            // means either the trie is empty or only a single leaf has been added previously.
            let curr_branch = match self.branch_stack.last_mut() {
                Some(curr_branch) => curr_branch,
                None if self.child_stack.is_empty() => {
                    // If the child stack is empty then this is the first leaf, push it and be done
                    self.child_stack
                        .push(ProofTrieBranchChild::Leaf { short_key: key, value: val });
                    return Ok(())
                }
                None => {
                    // If the child stack is not empty then it must only have a single other child
                    // which is either a leaf or extension with a non-zero short key.
                    debug_assert_eq!(self.child_stack.len(), 1);
                    debug_assert!(!self
                        .child_stack
                        .last()
                        .expect("already checked for emptiness")
                        .short_key()
                        .is_empty());
                    self.push_new_branch(key, val);
                    return Ok(())
                }
            };

            // Find the common prefix length, which is the number of nibbles shared between the
            // current branch and the key.
            let common_prefix_len = self.branch_path.common_prefix_length(&key);

            // If the current branch does not share all of its nibbles with the new key then it is
            // not the parent of the new key. In this case the current branch will have no more
            // children. We can pop it and loop back to the top to try again with its parent branch.
            if common_prefix_len < self.branch_path.len() {
                self.pop_branch()?;
                continue
            }

            // If the current branch is a prefix of the new key then the leaf is a child of the
            // branch. If the branch doesn't have the leaf's nibble set then the leaf can be added
            // directly, otherwise a new branch must be created in-between this branch and that
            // existing child.
            let nibble = key.get_unchecked(common_prefix_len);
            if curr_branch.state_mask.is_bit_set(nibble) {
                // This method will also push the new leaf onto the `child_stack`.
                self.push_new_branch(key, val);
            } else {
                curr_branch.state_mask.set_bit(nibble);

                // Push the leaf onto the stack.
                self.child_stack.push(ProofTrieBranchChild::Leaf {
                    short_key: key.slice_unchecked(common_prefix_len + 1, key.len()),
                    value: val,
                });
            }

            return Ok(())
        }
    }

    /// Internal implementation of proof calculation. Assumes both cursors have already been reset.
    /// See docs on [`Self::proof`] for expected behavior.
    fn proof_inner(
        &mut self,
        value_encoder: &VE,
        targets: impl IntoIterator<Item = Nibbles>,
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
        trace!(target: TRACE_TARGET, "proof_inner called");

        // In debug builds, verify that targets are sorted
        #[cfg(debug_assertions)]
        let targets = {
            let mut prev: Option<Nibbles> = None;
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

        // Ensure initial state is cleared. By the end of the method call these should be empty once
        // again.
        debug_assert!(self.branch_stack.is_empty());
        debug_assert!(self.branch_path.is_empty());
        debug_assert!(self.child_stack.is_empty());

        // Silence unused variable warning for now
        let _ = targets;

        let mut proof_nodes = Vec::new();
        let mut hashed_cursor_current = self.hashed_cursor.seek(B256::ZERO)?;
        loop {
            trace!(target: TRACE_TARGET, ?hashed_cursor_current, "proof_inner loop");

            // Fetch the next leaf from the hashed cursor, converting the key to Nibbles and
            // immediately creating the DeferredValueEncoder so that encoding of the leaf value can
            // begin ASAP.
            let Some((key, val)) = hashed_cursor_current.map(|(key_b256, val)| {
                debug_assert_eq!(key_b256.len(), 32);
                // SAFETY: key is a B256 and so is exactly 32-bytes.
                let key = unsafe { Nibbles::unpack_unchecked(key_b256.as_slice()) };
                let val = value_encoder.deferred_encoder(key_b256, val);
                (key, val)
            }) else {
                break
            };

            self.add_leaf(key, val)?;
            hashed_cursor_current = self.hashed_cursor.next()?;
        }

        // Once there's no more leaves we can pop the remaining branches, if any.
        while !self.branch_stack.is_empty() {
            self.pop_branch()?;
        }

        // At this point the branch stack should be empty. If the child stack is empty it means no
        // keys were ever iterated from the hashed cursor in the first place. Otherwise there should
        // only be a single node left: the root node.
        debug_assert!(self.branch_stack.is_empty());
        debug_assert!(self.branch_path.is_empty());
        debug_assert!(self.child_stack.len() < 2);

        // Determine the root node based on the child stack, and push the proof of the root node
        // onto the result stack.
        let root_node = if let Some(node) = self.child_stack.pop() {
            node.into_trie_node(&mut self.rlp_encode_buf)?
        } else {
            TrieNode::EmptyRoot
        };

        proof_nodes.push(ProofTrieNode {
            path: Nibbles::new(), // root path
            node: root_node,
            masks: TrieMasks::none(),
        });

        Ok(proof_nodes)
    }
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: LeafValueEncoder<Value = HC::Value>,
{
    /// Generate a proof for the given targets.
    ///
    /// Given lexicographically sorted targets, returns nodes whose paths are a prefix of any
    /// target. The returned nodes will be sorted lexicographically by path.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    pub fn proof(
        &mut self,
        value_encoder: &VE,
        targets: impl IntoIterator<Item = Nibbles>,
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
        self.trie_cursor.reset();
        self.hashed_cursor.reset();
        self.proof_inner(value_encoder, targets)
    }
}

/// A proof calculator for storage tries.
pub type StorageProofCalculator<TC, HC> = ProofCalculator<TC, HC, StorageValueEncoder>;

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
    /// Given lexicographically sorted targets, returns nodes whose paths are a prefix of any
    /// target. The returned nodes will be sorted lexicographically by path.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self, targets))]
    pub fn storage_proof(
        &mut self,
        hashed_address: B256,
        targets: impl IntoIterator<Item = Nibbles>,
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
        /// Static storage value encoder instance used by all storage proofs.
        static STORAGE_VALUE_ENCODER: StorageValueEncoder = StorageValueEncoder;

        self.hashed_cursor.set_hashed_address(hashed_address);

        // Shortcut: check if storage is empty
        if self.hashed_cursor.is_storage_empty()? {
            // Return a single EmptyRoot node at the root path
            return Ok(vec![ProofTrieNode {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
        proof::Proof,
        trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
    };
    use alloy_primitives::map::B256Map;
    use alloy_rlp::Decodable;
    use itertools::Itertools;
    use reth_trie_common::{HashedPostState, MultiProofTargets};
    use std::collections::BTreeMap;

    /// Target to use with the `tracing` crate.
    static TRACE_TARGET: &str = "trie::proof_v2::tests";

    /// A test harness for comparing `ProofCalculator` and legacy `Proof` implementations.
    ///
    /// This harness creates mock cursor factories from a `HashedPostState` and provides
    /// a method to test that both proof implementations produce equivalent results.
    struct ProofTestHarness {
        /// Mock factory for trie cursors (empty by default for leaf-only tests)
        trie_cursor_factory: MockTrieCursorFactory,
        /// Mock factory for hashed cursors, populated from `HashedPostState`
        hashed_cursor_factory: MockHashedCursorFactory,
    }

    impl ProofTestHarness {
        /// Creates a new test harness from a `HashedPostState`.
        ///
        /// The `HashedPostState` is used to populate the mock hashed cursor factory directly.
        /// The trie cursor factory is empty by default, suitable for testing the leaf-only
        /// proof calculator.
        fn new(post_state: HashedPostState) -> Self {
            trace!(target: TRACE_TARGET, ?post_state, "Creating ProofTestHarness");

            // Extract accounts from post state, filtering out None (deleted accounts)
            let hashed_accounts: BTreeMap<B256, _> = post_state
                .accounts
                .into_iter()
                .filter_map(|(addr, account)| account.map(|acc| (addr, acc)))
                .collect();

            // Extract storage tries from post state
            let hashed_storage_tries: B256Map<BTreeMap<B256, U256>> = post_state
                .storages
                .into_iter()
                .map(|(addr, hashed_storage)| {
                    // Convert HashedStorage to BTreeMap, filtering out zero values (deletions)
                    let storage_map: BTreeMap<B256, U256> = hashed_storage
                        .storage
                        .into_iter()
                        .filter_map(|(slot, value)| (value != U256::ZERO).then_some((slot, value)))
                        .collect();
                    (addr, storage_map)
                })
                .collect();

            // Ensure that there's a storage trie dataset for every storage trie, even if empty.
            let storage_trie_nodes: B256Map<BTreeMap<_, _>> = hashed_storage_tries
                .keys()
                .copied()
                .map(|addr| (addr, Default::default()))
                .collect();

            // Create mock hashed cursor factory populated with the post state data
            let hashed_cursor_factory =
                MockHashedCursorFactory::new(hashed_accounts, hashed_storage_tries);

            // Create empty trie cursor factory (leaf-only calculator doesn't need trie nodes)
            let trie_cursor_factory =
                MockTrieCursorFactory::new(BTreeMap::new(), storage_trie_nodes);

            Self { trie_cursor_factory, hashed_cursor_factory }
        }

        /// Asserts that `ProofCalculator` and legacy `Proof` produce equivalent results for account
        /// proofs.
        ///
        /// This method calls both implementations with the given account targets and compares
        /// the results. For now, it performs a basic comparison by checking that both succeed
        /// and produce non-empty results. More detailed comparison logic can be added as needed.
        fn assert_proof(
            &self,
            // For now ProofCalculator doesn't support real targets, we just compare calculated
            // roots.
            _targets: impl IntoIterator<Item = B256> + Clone,
        ) -> Result<(), StateProofError> {
            // Create ProofCalculator (proof_v2) with account cursors
            let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;
            let hashed_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;

            // Call ProofCalculator::proof with account targets
            let value_encoder = SyncAccountValueEncoder::new(
                self.trie_cursor_factory.clone(),
                self.hashed_cursor_factory.clone(),
            );
            let mut proof_calculator = ProofCalculator::new(trie_cursor, hashed_cursor);
            let proof_v2_result = proof_calculator.proof(&value_encoder, [Nibbles::new()])?;

            // Call Proof::multiproof (legacy implementation)
            let proof_legacy_result =
                Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                    .multiproof(MultiProofTargets::default())?;

            // Decode and sort legacy proof nodes
            let proof_legacy_nodes = proof_legacy_result
                .account_subtree
                .iter()
                .map(|(path, node_enc)| {
                    let mut buf = node_enc.as_ref();
                    let node = TrieNode::decode(&mut buf)
                        .expect("legacy implementation should not produce malformed proof nodes");

                    ProofTrieNode {
                        path: *path,
                        node,
                        masks: TrieMasks {
                            hash_mask: proof_legacy_result
                                .branch_node_hash_masks
                                .get(path)
                                .copied(),
                            tree_mask: proof_legacy_result
                                .branch_node_tree_masks
                                .get(path)
                                .copied(),
                        },
                    }
                })
                .sorted_by_key(|n| n.path)
                .collect::<Vec<_>>();

            // Basic comparison: both should succeed and produce identical results
            assert_eq!(proof_legacy_nodes, proof_v2_result);

            Ok(())
        }
    }

    mod proptest_tests {
        use super::*;
        use alloy_primitives::{map::B256Map, U256};
        use proptest::prelude::*;
        use reth_primitives_traits::Account;
        use reth_trie_common::HashedPostState;

        /// Generate a strategy for Account values
        fn account_strategy() -> impl Strategy<Value = Account> {
            (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(
                |(nonce, balance, code_hash)| Account {
                    nonce,
                    balance: U256::from(balance),
                    bytecode_hash: Some(B256::from(code_hash)),
                },
            )
        }

        /// Generate a strategy for `HashedPostState` with random accounts
        fn hashed_post_state_strategy() -> impl Strategy<Value = HashedPostState> {
            prop::collection::vec((any::<[u8; 32]>(), account_strategy()), 0..20).prop_map(
                |accounts| {
                    let account_map = accounts
                        .into_iter()
                        .map(|(addr_bytes, account)| (B256::from(addr_bytes), Some(account)))
                        .collect::<B256Map<_>>();

                    // All accounts have empty storages.
                    let storages = account_map
                        .keys()
                        .copied()
                        .map(|addr| (addr, Default::default()))
                        .collect::<B256Map<_>>();

                    HashedPostState { accounts: account_map, storages }
                },
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(5000))]

            /// Tests that ProofCalculator produces valid proofs for randomly generated
            /// HashedPostState with empty target sets.
            ///
            /// This test:
            /// - Generates random accounts in a HashedPostState
            /// - Creates a test harness with the generated state
            /// - Calls assert_proof with an empty target set
            /// - Verifies both ProofCalculator and legacy Proof succeed
            #[test]
            fn proptest_proof_with_empty_targets(
                post_state in hashed_post_state_strategy(),
            ) {
                reth_tracing::init_test_tracing();
                let harness = ProofTestHarness::new(post_state);

                // Pass empty target set
                harness.assert_proof(std::iter::empty()).expect("Proof generation failed");
            }
        }
    }
}
