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
    trie_cursor::{depth_first, TrieCursor, TrieStorageCursor},
};
use alloy_primitives::{keccak256, B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    prefix_set::PrefixSet, BranchNodeMasks, BranchNodeRef, BranchNodeV2, Nibbles, ProofTrieNodeV2,
    ProofV2Target, RlpNode, TrieNodeV2,
};
use std::{cmp::Ordering, sync::Arc};
use tracing::{error, instrument, trace};

mod value;
pub use value::*;

mod node;
use node::*;

mod target;
pub(crate) use target::*;

/// Target to use with the `tracing` crate.
static TRACE_TARGET: &str = "trie::proof_v2";

/// Number of bytes to pre-allocate for [`ProofCalculator`]'s `rlp_encode_buf` field.
const RLP_ENCODE_BUF_SIZE: usize = 1024;

/// A proof calculator that generates merkle proofs using only leaf data.
///
/// The calculator:
/// - Accepts one or more B256 proof targets sorted lexicographically
/// - Returns proof nodes sorted lexicographically by path
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
    /// one, and the branch is pushed onto this stack in their place (see [`Self::pop_branch`].
    ///
    /// Given that keys are consumed in lexicographical order, only the last child on the stack
    /// can still require structural changes (e.g. splitting its short key when inserting a new
    /// branch). Once a child is finalized, [`Self::commit_last_child`] converts it to a
    /// [`ProofTrieBranchChild::RlpNode`] if retained for the proof. Unretained children can remain
    /// unencoded anywhere on the stack until [`Self::pop_branch`], giving deferred encoders more
    /// time for async work.
    child_stack: Vec<ProofTrieBranchChild<VE::DeferredEncoder>>,
    /// Cached branch data pulled from the `trie_cursor`. The calculator will use the cached
    /// [`BranchNodeCompact::hashes`] to skip over the calculation of sub-tries in the overall
    /// trie. The cached hashes cannot be used for any paths which are prefixes of a proof target.
    cached_branch_stack: Vec<(Nibbles, BranchNodeCompact)>,
    /// The proofs which will be returned from the calculation. This gets taken at the end of every
    /// proof call.
    retained_proofs: Vec<ProofTrieNodeV2>,
    /// Free-list of re-usable buffers of [`RlpNode`]s, used for encoding branch nodes to RLP.
    ///
    /// We are generally able to re-use these buffers across different branch nodes for the
    /// duration of a proof calculation, but occasionally we will lose one when a branch
    /// node is returned as a `ProofTrieNode`.
    rlp_nodes_bufs: Vec<Vec<RlpNode>>,
    /// Re-usable byte buffer, used for RLP encoding.
    rlp_encode_buf: Vec<u8>,
    /// Prefix set for tracking changed keys.
    prefix_set: PrefixSet,
}

impl<TC, HC, VE: LeafValueEncoder> ProofCalculator<TC, HC, VE> {
    /// Create a new [`ProofCalculator`] instance for calculating account proofs.
    pub fn new(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self {
            trie_cursor,
            hashed_cursor,
            branch_stack: Vec::<_>::with_capacity(64),
            branch_path: Nibbles::new(),
            child_stack: Vec::<_>::with_capacity(64),
            cached_branch_stack: Vec::<_>::with_capacity(64),
            retained_proofs: Vec::<_>::with_capacity(32),
            rlp_nodes_bufs: Vec::<_>::with_capacity(8),
            rlp_encode_buf: Vec::<_>::with_capacity(RLP_ENCODE_BUF_SIZE),
            prefix_set: PrefixSet::default(),
        }
    }

    /// Sets the prefix set and returns `self`.
    ///
    /// When given, all cached hashes matching the [`PrefixSet`] will be invalidated and their
    /// subtries recalculated.
    pub fn with_prefix_set(mut self, prefix_set: PrefixSet) -> Self {
        self.prefix_set = prefix_set;
        self
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

    // Returns zero if `branch_stack` is empty, one otherwise.
    //
    // This is used when working with the `ext_len` field of [`ProofTrieBranch`]. The `ext_len` is
    // calculated by taking the difference of the current `branch_path` and the new branch's path;
    // if the new branch has a parent branch (ie `branch_stack` is not empty) then 1 is subtracted
    // from the `ext_len` to account for the child's nibble on the parent.
    #[inline]
    const fn maybe_parent_nibble(&self) -> usize {
        !self.branch_stack.is_empty() as usize
    }

    /// Returns true if the proof of a node at the given path should be retained. A node is retained
    /// if its path is a prefix of any target.
    ///
    /// This may move the `targets` iterator forward if the given path comes after the current
    /// target.
    ///
    /// This method takes advantage of the [`std::slice::Iter`] component of [`TargetsCursor`] to
    /// check the minimum number of targets. In general it looks at a current target and the next
    /// target simultaneously, forming an end-exclusive range.
    ///
    /// ```text
    /// * Given targets: [ 0x012, 0x045, 0x678 ]
    /// * targets.current() returns:
    ///     - (0x012, Some(0x045)): covers (0x012..0x045)
    ///     - (0x045, Some(0x678)): covers (0x045..0x678)
    ///     - (0x678, None): covers (0x678..)
    /// ```
    ///
    /// As long as the path which is passed in lies within that range we can continue to use the
    /// current target. Once the path goes beyond that range (ie path >= next target) then we can be
    /// sure that no further paths will be in the range, and we can iterate forward.
    ///
    /// ```text
    /// * Given:
    ///     - path: 0x04
    ///     - targets.current() returns (0x012, Some(0x045))
    ///
    /// * 0x04 comes _after_ 0x045 in depth-first order, so (0x012..0x045) does not contain 0x04.
    ///
    /// * targets.next() is called.
    ///
    /// * targets.current() now returns (0x045, Some(0x678)). This does contain 0x04.
    ///
    /// * 0x04 is a prefix of 0x045, and so is retained.
    /// ```
    #[instrument(
        target = TRACE_TARGET,
        level = "trace",
        skip_all,
        fields(?path, ?check_parent_path),
        ret,
    )]
    fn should_retain<'a>(
        &self,
        targets: &mut Option<TargetsCursor<'a>>,
        path: &Nibbles,
        check_parent_path: bool,
    ) -> bool {
        // If no targets are given then we never retain anything
        let Some(targets) = targets.as_mut() else { return false };

        let (mut lower, mut upper) = targets.current();

        loop {
            // If the node in question is a prefix of the target then we do not iterate targets
            // further.
            //
            // Even if the node is a prefix of the target's key, a target with a parent path only
            // retains nodes strictly below that already-revealed parent.
            //
            // _However_ even if the node doesn't match one target due to its parent path, it may
            // match other targets whose keys match this node. So we search forwards and backwards
            // for all targets which might match this node.
            //
            // For example, given a branch 0xabc, with children at 0, 1, and 2, and targets:
            // - key: 0xabc0, parent path length: 1
            // - key: 0xabc1, parent path length: 0
            // - key: 0xabc2, parent path length: 3 <-- current
            // - key: 0xabc3, parent path length: 2
            //
            // When the branch node at 0xabc is visited it will be after the targets has iterated
            // forward to 0xabc2 (because all children will have been visited already). At this
            // point the target for 0xabc2 will not match the branch due to its prefix, but any of
            // the other targets would, so we need to check those as well.
            if lower.key_nibbles.starts_with(path) {
                let is_below_parent = |target: &ProofV2Target| {
                    target.parent.path_len().is_none_or(|len| path.len() > len)
                };
                return !check_parent_path ||
                    (is_below_parent(lower) ||
                        targets
                            .skip_iter()
                            .take_while(|target| target.key_nibbles.starts_with(path))
                            .any(is_below_parent) ||
                        targets
                            .rev_iter()
                            .take_while(|target| target.key_nibbles.starts_with(path))
                            .any(is_below_parent))
            }

            // If the path isn't in the current range then iterate forward until it is (or until
            // there is no upper bound, indicating unbounded).
            if upper
                .is_some_and(|upper| depth_first::cmp(path, &upper.key_nibbles) != Ordering::Less)
            {
                (lower, upper) = targets.next();
                trace!(target: TRACE_TARGET, target = ?lower, "upper target <= path, next target");
            } else {
                return false
            }
        }
    }

    /// Takes a child which has been removed from the `child_stack` and converts it to an
    /// [`RlpNode`], retaining it as a proof node.
    ///
    /// Calling this method indicates that the child will not undergo any further modifications.
    ///
    /// The caller has already determined via [`Self::should_retain`] that this child is retained.
    /// That decision must not be re-evaluated here, because `should_retain` advances the `targets`
    /// cursor. Children which are not retained are converted later, in [`Self::pop_branch`].
    fn commit_child(
        &mut self,
        child_path: Nibbles,
        child: ProofTrieBranchChild<VE::DeferredEncoder>,
    ) -> Result<RlpNode, StateProofError> {
        // An already encoded child only needs an extension if it has been rebased upward.
        if matches!(&child, ProofTrieBranchChild::RlpNode { .. }) {
            self.rlp_encode_buf.clear();
            return child.into_rlp(&mut self.rlp_encode_buf).map(|(node, _)| node)
        }

        trace!(target: TRACE_TARGET, ?child_path, "Retaining child");

        // Convert to `ProofTrieNodeV2`, which will be what is retained.
        //
        // If this node is a branch then its `rlp_nodes_buf` will be taken and not returned to the
        // `rlp_nodes_bufs` free-list.
        self.rlp_encode_buf.clear();
        let proof_node = child.into_proof_trie_node(child_path, &mut self.rlp_encode_buf)?;

        // Use the `ProofTrieNodeV2` to encode the `RlpNode`, and then push it onto retained nodes
        // before returning.
        self.rlp_encode_buf.clear();
        proof_node.node.encode(&mut self.rlp_encode_buf);

        self.retained_proofs.push(proof_node);
        Ok(RlpNode::from_rlp(&self.rlp_encode_buf))
    }

    /// Returns the path of the child of the currently under-construction branch at the given
    /// nibble.
    #[inline]
    fn child_path_at(&self, nibble: u8) -> Nibbles {
        let mut child_path = self.branch_path;
        debug_assert!(child_path.len() < 64);
        child_path.push_unchecked(nibble);
        child_path
    }

    /// Returns index of the highest nibble which is set in the mask.
    ///
    /// # Panics
    ///
    /// Will panic in debug mode if the mask is empty.
    #[inline]
    fn highest_set_nibble(mask: TrieMask) -> u8 {
        debug_assert!(!mask.is_empty());
        (u16::BITS - mask.leading_zeros() - 1) as u8
    }

    /// Returns the path of the child on top of the `child_stack`, or the root path if the stack is
    /// empty. Returns None if the current branch has not yet pushed a child (empty `state_mask`).
    fn last_child_path(&self) -> Option<Nibbles> {
        // If there is no branch under construction then the top child must be the root child.
        let Some(branch) = self.branch_stack.last() else {
            return Some(Nibbles::new());
        };

        (!branch.state_mask.is_empty())
            .then(|| self.child_path_at(Self::highest_set_nibble(branch.state_mask)))
    }

    /// If the last child of `child_stack` is retained for the proof, calls [`Self::commit_child`]
    /// to replace it with a [`ProofTrieBranchChild::RlpNode`]. Otherwise, leaves it unencoded until
    /// [`Self::pop_branch`].
    ///
    /// If `child_stack` is empty then this is a no-op.
    ///
    /// NOTE that this method call relies on the `state_mask` of the top branch of the
    /// `branch_stack` to determine the last child's path. When committing the last child prior to
    /// pushing a new child, it's important to set the new child's `state_mask` bit _after_ the call
    /// to this method.
    #[instrument(
        target = TRACE_TARGET,
        level = "trace",
        skip_all,
        fields(child_path = ?self.last_child_path()),
    )]
    fn commit_last_child<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
    ) -> Result<(), StateProofError> {
        if matches!(self.child_stack.last(), Some(ProofTrieBranchChild::RlpNode { .. })) {
            trace!(target: TRACE_TARGET, "Last child already committed, leaving stack unchanged");
            return Ok(())
        }

        let Some(child_path) = self.last_child_path() else { return Ok(()) };
        let child =
            self.child_stack.pop().expect("child_stack can't be empty if there's a child path");

        // Only commit immediately if retained for the proof. Otherwise, defer conversion
        // to pop_branch() to give DeferredEncoder time for async work.
        if self.should_retain(targets, &child_path, true) {
            let (hash_mask_bit, tree_mask_bit) = child.mask_bits();
            let child_rlp_node = self.commit_child(child_path, child)?;
            trace!(target: TRACE_TARGET, ?child_rlp_node, "Pushing committed child RlpNode onto stack");
            self.child_stack.push(ProofTrieBranchChild::RlpNode {
                node: child_rlp_node,
                short_key: Nibbles::new(),
                hash_mask_bit,
                tree_mask_bit,
            });
        } else {
            trace!(target: TRACE_TARGET, "Pushing uncommitted child onto stack");
            self.child_stack.push(child);
        }

        Ok(())
    }

    /// Adds a child at its full path, possibly collapsing an existing branch and/or creating a new
    /// one depending on the path.
    fn push_child<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        mut child: ProofTrieBranchChild<VE::DeferredEncoder>,
    ) -> Result<(), StateProofError> {
        let path = *child.short_key();

        loop {
            trace!(
                target: TRACE_TARGET,
                ?path,
                branch_stack_len = ?self.branch_stack.len(),
                branch_path = ?self.branch_path,
                child_stack_len = ?self.child_stack.len(),
                "push_child: loop",
            );

            // Get the `state_mask` of the branch currently being built. If there are no branches
            // on the stack then the trie is either empty or contains only a single child.
            let (nibble, short_key) = match self.branch_stack.last().map(|branch| branch.state_mask)
            {
                None if self.child_stack.is_empty() => {
                    // The first child is the root and already has the correct short key.
                    self.child_stack.push(child);
                    return Ok(())
                }
                None => {
                    // Split the existing root child from the new child by their common prefix.
                    debug_assert_eq!(self.child_stack.len(), 1);
                    debug_assert!(!self
                        .child_stack
                        .last()
                        .expect("already checked for emptiness")
                        .short_key()
                        .is_empty());
                    self.push_new_branch(path)
                }
                Some(state_mask) => {
                    // Find the number of nibbles shared by the current branch and the new child.
                    let common_prefix_len = self.branch_path.common_prefix_length(&path);

                    // A branch which is not a prefix of the child cannot receive any more
                    // children, so finish it and try again with its parent.
                    if common_prefix_len < self.branch_path.len() {
                        self.pop_branch(targets)?;
                        continue
                    }

                    // If this nibble is already occupied, split the existing child from the new
                    // child. Otherwise the new child can be inserted directly into this branch.
                    let nibble = path.get_unchecked(common_prefix_len);
                    if state_mask.is_bit_set(nibble) {
                        self.push_new_branch(path)
                    } else {
                        (nibble, trim_nibbles_prefix(&path, common_prefix_len + 1))
                    }
                }
            };

            // Store the child key relative to its new parent branch.
            child.trim_short_key_prefix(path.len() - short_key.len());

            // The preceding child is structurally final. Commit it if retained for the proof;
            // otherwise its encoding can wait until pop_branch.
            self.commit_last_child(targets)?;

            let branch = self.branch_stack.last_mut().expect("branch_stack cannot be empty");
            debug_assert!(!branch.state_mask.is_bit_set(nibble));

            // Set the new child's bit after commit_last_child, which uses the mask to find the
            // preceding child's path.
            branch.state_mask.set_bit(nibble);

            // The parent now contains this child at `nibble`.
            self.child_stack.push(child);
            return Ok(())
        }
    }

    /// Pushes a new branch onto the `branch_stack` based on the path and short key of the last
    /// child on the `child_stack` and the path of the next child which will be pushed on to the
    /// stack after this call.
    ///
    /// Returns the nibble of the branch's `state_mask` which should be set for the new child, and
    /// the short key that child should use.
    fn push_new_branch(&mut self, new_child_path: Nibbles) -> (u8, Nibbles) {
        // Get the full path to the root of the existing child. The trie root is used when there is
        // no parent branch.
        let first_child_path = self
            .last_child_path()
            .expect("push_new_branch requires the current branch to have a child");

        // Put both children in the same coordinate system by trimming the existing child's root
        // path from the new child's full path.
        let new_child_short_key = trim_nibbles_prefix(&new_child_path, first_child_path.len());
        let first_child_short_key = *self
            .child_stack
            .last()
            .expect("push_new_branch can't be called with empty child_stack")
            .short_key();
        debug_assert!(!first_child_short_key.is_empty());

        // Their shared prefix becomes the new branch's extension. The first differing nibble from
        // each key becomes that child's position in the branch.
        let common_prefix_len = first_child_short_key.common_prefix_length(&new_child_short_key);
        let first_child_nibble = first_child_short_key.get_unchecked(common_prefix_len);
        let new_child_nibble = new_child_short_key.get_unchecked(common_prefix_len);

        // Remove the extension and branch nibble from the new child's remaining short key.
        let new_child_short_key = trim_nibbles_prefix(&new_child_short_key, common_prefix_len + 1);

        // The new branch starts after the existing child's parent path and their shared extension.
        let branch_path_len = first_child_path.len() + common_prefix_len;
        self.branch_path = new_child_path.slice_unchecked(0, branch_path_len);

        // Rebase the existing child beneath the new branch by removing the extension and its
        // branch nibble from the front of its short key.
        let first_child = self
            .child_stack
            .last_mut()
            .expect("push_new_branch can't be called with empty child_stack");
        first_child.trim_short_key_prefix(common_prefix_len + 1);

        // Push the new branch onto the `branch_stack`. We do not yet set the `state_mask` bit of
        // the new child; whatever actually pushes the child onto the `child_stack` is expected to
        // do that.
        self.branch_stack.push(ProofTrieBranch {
            ext_len: common_prefix_len as u8,
            state_mask: TrieMask::new(1 << first_child_nibble),
        });

        trace!(
            target: TRACE_TARGET,
            ?new_child_path,
            ext_len = ?common_prefix_len,
            ?first_child_nibble,
            branch_path = ?self.branch_path,
            "Pushed new branch",
        );

        (new_child_nibble, new_child_short_key)
    }

    /// Pops the top branch off of the `branch_stack`, hashes its children on the `child_stack`, and
    /// replaces those children on the `child_stack`. The `branch_path` field will be updated
    /// accordingly.
    ///
    /// # Panics
    ///
    /// This method panics if `branch_stack` is empty.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    fn pop_branch<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
    ) -> Result<(), StateProofError> {
        trace!(
            target: TRACE_TARGET,
            branch = ?self.branch_stack.last(),
            branch_path = ?self.branch_path,
            child_stack_len = ?self.child_stack.len(),
            "called",
        );

        // Retain the final child if needed before draining the branch's children. Unretained
        // children are encoded below.
        self.commit_last_child(targets)?;

        let mut rlp_nodes_buf = self.take_rlp_nodes_buf();
        let mut masks = BranchNodeMasks::default();
        let branch = self.branch_stack.pop().expect("branch_stack cannot be empty");

        // Take the branch's children off the stack, using the state mask to determine how many
        // there are.
        let num_children = branch.state_mask.count_ones() as usize;
        debug_assert!(
            self.child_stack.len() >= num_children,
            "Stack is missing necessary children ({num_children:?})"
        );
        debug_assert!(
            num_children >= 2,
            "A branch must have at least two children, got {num_children}"
        );

        // Collect children into RlpNode Vec. Children are in lexicographic order.
        rlp_nodes_buf.reserve(num_children);
        for (nibble, child) in branch
            .state_mask
            .iter()
            .zip(self.child_stack.drain(self.child_stack.len() - num_children..))
        {
            let (hash_mask_bit, tree_mask_bit) = child.mask_bits();
            masks.set_child_bits(nibble, hash_mask_bit, tree_mask_bit);

            self.rlp_encode_buf.clear();
            let (child_rlp_node, freed_buf) = child.into_rlp(&mut self.rlp_encode_buf)?;
            if let Some(buf) = freed_buf {
                self.rlp_nodes_bufs.push(buf);
            }
            rlp_nodes_buf.push(child_rlp_node);
        }

        debug_assert_eq!(
            rlp_nodes_buf.len(),
            num_children,
            "children length must match number of bits set in state_mask"
        );

        // Calculate the short key of the parent extension (if the branch has a parent extension).
        // It's important to calculate this short key prior to modifying the `branch_path`.
        let short_key = trim_nibbles_prefix(
            &self.branch_path,
            self.branch_path.len() - branch.ext_len as usize,
        );

        // Compute hash for the branch node if it has a parent extension.
        let rlp_node = if short_key.is_empty() {
            None
        } else {
            self.rlp_encode_buf.clear();
            BranchNodeRef::new(&rlp_nodes_buf, branch.state_mask).encode(&mut self.rlp_encode_buf);
            Some(RlpNode::from_rlp(&self.rlp_encode_buf))
        };

        // Update the branch_path. If this branch is the only branch then only its extension needs
        // to be trimmed, otherwise we also need to remove its nibble from its parent.
        let new_path_len =
            self.branch_path.len() - branch.ext_len as usize - self.maybe_parent_nibble();

        // Wrap the `BranchNodeV2` so it can be pushed onto the child stack.
        let branch_as_child = ProofTrieBranchChild::Branch {
            node: BranchNodeV2::new(short_key, rlp_nodes_buf, branch.state_mask, rlp_node),
            masks: (!masks.is_empty()).then_some(masks),
        };

        debug_assert!(self.branch_path.len() >= new_path_len);
        self.branch_path = self.branch_path.slice_unchecked(0, new_path_len);

        self.child_stack.push(branch_as_child);

        Ok(())
    }

    /// Given the lower and upper bounds (exclusive) of a range of keys, iterates over the
    /// `hashed_cursor` and calculates all trie nodes possible based on those keys. If the upper
    /// bound is None then it is considered unbounded.
    ///
    /// It is expected that this method is "driven" by `next_uncached_key_range`, which decides
    /// which ranges of keys need to be calculated based on what cached trie data is available.
    #[instrument(
        target = TRACE_TARGET,
        level = "trace",
        skip_all,
        fields(?lower_bound, ?upper_bound),
    )]
    fn calculate_key_range<'a>(
        &mut self,
        value_encoder: &mut VE,
        targets: &mut Option<TargetsCursor<'a>>,
        hashed_cursor_state: &mut HashedCursorState<VE::DeferredEncoder>,
        lower_bound: Nibbles,
        upper_bound: Option<Nibbles>,
    ) -> Result<(), StateProofError> {
        // A helper closure for mapping entries returned from the `hashed_cursor`, converting the
        // key to Nibbles and immediately creating the DeferredValueEncoder so that encoding of the
        // leaf value can begin ASAP.
        let mut map_hashed_cursor_entry = |(key_b256, val): (B256, _)| {
            debug_assert_eq!(key_b256.len(), 32);
            let key = Nibbles::unpack_array(key_b256.as_ref());
            let val = value_encoder.deferred_encoder(key_b256, val);
            (key, val)
        };

        // If the cursor hasn't been used, or the last iterated key is prior to this range's key
        // range, then seek forward to at least the first key.
        if hashed_cursor_state.needs_seek_to(&lower_bound) {
            trace!(
                target: TRACE_TARGET,
                current=?hashed_cursor_state.path(),
                "Seeking hashed cursor to meet lower bound",
            );

            let lower_key = B256::right_padding_from(&lower_bound.pack());
            *hashed_cursor_state = HashedCursorState::seeked(
                lower_bound,
                self.hashed_cursor.seek(lower_key)?.map(&mut map_hashed_cursor_entry),
            );
        }

        // Loop over all keys in the range, pushing each leaf onto the stack.
        while hashed_cursor_state
            .path()
            .is_some_and(|key| upper_bound.is_none_or(|upper_bound| key < &upper_bound))
        {
            let (key, val) = hashed_cursor_state.take();
            self.push_child(targets, ProofTrieBranchChild::Leaf { short_key: key, value: val })?;
            *hashed_cursor_state = HashedCursorState::seeked(
                key,
                self.hashed_cursor.next()?.map(&mut map_hashed_cursor_entry),
            );
        }

        trace!(target: TRACE_TARGET, "No further keys within range");
        Ok(())
    }

    /// Takes a cached branch from the trie cursor and invalidates its hashes if it may collapse.
    fn take_cached_branch(
        &mut self,
        trie_cursor_state: &mut TrieCursorState,
    ) -> (Nibbles, BranchNodeCompact) {
        let (cached_path, mut cached_branch) = trie_cursor_state.take();

        if self.prefix_set.contains(&cached_path) {
            let mut unchanged_children = 0;
            let mut child_path = cached_path;
            for nibble in cached_branch.state_mask.iter() {
                child_path.truncate(cached_path.len());
                child_path.push_unchecked(nibble);
                if !self.prefix_set.contains(&child_path) {
                    unchanged_children += 1;
                    if unchanged_children > 1 {
                        break
                    }
                }
            }

            // If every dirty child is deleted, a lone unchanged child must be revealed so the
            // branch can collapse into it. Keep the masks as structural hints, but prevent any of
            // this branch's cached hashes from hiding that child.
            if unchanged_children <= 1 {
                Arc::make_mut(&mut cached_branch.hashes).fill(B256::ZERO);
                trace!(
                    target: TRACE_TARGET,
                    ?cached_path,
                    ?unchanged_children,
                    "Invalidated cached hashes because branch may collapse",
                );
            }
        }

        (cached_path, cached_branch)
    }

    /// Attempts to pop off the top branch of the `cached_branch_stack`, returning
    /// [`PopCachedBranchOutcome::Popped`] on success. Returns other variants to indicate that the
    /// stack is empty and what to do about it.
    ///
    /// This method only returns [`PopCachedBranchOutcome::CalculateLeaves`] if there is a cached
    /// branch on top of the stack.
    #[inline]
    fn try_pop_cached_branch(
        &mut self,
        trie_cursor_state: &mut TrieCursorState,
        traversal_upper_bound: Option<&Nibbles>,
        uncalculated_lower_bound: &Option<Nibbles>,
    ) -> Result<PopCachedBranchOutcome, StateProofError> {
        // If the `uncalculated_lower_bound` is None it indicates that there can be no more
        // leaf data, so similarly there can be no more cached branch data.
        let Some(uncalculated_lower_bound) = uncalculated_lower_bound else {
            return Ok(PopCachedBranchOutcome::Exhausted)
        };

        // Use the branch on top of the stack unless a previously calculated range has already
        // covered its entire subtrie.
        while let Some(cached) = self.cached_branch_stack.pop() {
            if cached
                .0
                .next_without_prefix()
                .is_some_and(|upper_bound| upper_bound <= *uncalculated_lower_bound)
            {
                trace!(target: TRACE_TARGET, cached_path=?cached.0, ?uncalculated_lower_bound, "Skipping covered cached branch");
                continue
            }
            return Ok(PopCachedBranchOutcome::Popped(cached));
        }

        // There is no cached branch on the stack. It's possible that another one exists
        // farther on in the trie, but we perform some checks first to prevent unnecessary
        // attempts to find it.

        // If [`TrieCursorState::path`] returns None it means that the cursor has been
        // exhausted, so there can be no more cached data.
        let Some(mut trie_cursor_path) = trie_cursor_state.path() else {
            return Ok(PopCachedBranchOutcome::Exhausted)
        };

        // If the trie cursor is seeked to a branch whose leaves have already been processed
        // then we can't use it, instead we seek forward and try again.
        if trie_cursor_path < uncalculated_lower_bound {
            *trie_cursor_state = TrieCursorState::seeked(
                *uncalculated_lower_bound,
                self.trie_cursor.seek(*uncalculated_lower_bound)?,
            );

            // Having just seeked forward we need to check if the cursor is now exhausted,
            // extracting the new path at the same time.
            if let Some(new_trie_cursor_path) = trie_cursor_state.path() {
                trie_cursor_path = new_trie_cursor_path
            } else {
                return Ok(PopCachedBranchOutcome::Exhausted)
            };
        }

        // If the trie cursor has reached the end of the traversal range then we consider cached
        // data to be exhausted. The cursor itself remains positioned for reuse by a later range.
        if traversal_upper_bound.is_some_and(|upper_bound| trie_cursor_path >= upper_bound) {
            return Ok(PopCachedBranchOutcome::Exhausted)
        }

        // At this point we can be sure that the cursor is in an `Available` state. We know for
        // sure it's not `Exhausted` because of the calls to `path` above, and we know it's not
        // `Taken` because we push all taken branches onto the `cached_branch_stack`, and the
        // stack is empty.
        //
        // We will use this `Available` cached branch as our next branch.
        let cached = self.take_cached_branch(trie_cursor_state);
        trace!(target: TRACE_TARGET, cached=?cached, "Pushed next trie node onto cached_branch_stack");

        // If the calculated range is not caught up to the next cached branch it means there
        // are portions of the trie prior to that branch which may need to be calculated;
        // return the uncalculated range up to that branch to make that happen.
        //
        // If the next cached branch's path is all zeros then we can skip this catch-up step,
        // because there cannot be any keys prior to that range.
        let cached_path = &cached.0;
        if uncalculated_lower_bound < cached_path && !cached_path.is_zeroes() {
            let range = (*uncalculated_lower_bound, Some(*cached_path));
            trace!(target: TRACE_TARGET, ?range, "Returning key range to calculate in order to catch up to cached branch");

            // Push the cached branch onto the stack so it's available once the leaf range is done
            // being calculated.
            self.cached_branch_stack.push(cached);

            return Ok(PopCachedBranchOutcome::CalculateLeaves(range));
        }

        Ok(PopCachedBranchOutcome::Popped(cached))
    }

    /// Pop any under-construction branches that are now complete. Assumes that all trie data prior
    /// to `next_path`, if any, has been computed. Any branches which were under-construction
    /// previously, and which do not share a prefix with `next_path`, can be assumed to be
    /// completed; they will not have any further keys added to them.
    ///
    /// Returns a range to calculate if a branch still has dirty keys to process, or popping it
    /// exposes dirty keys which could split its extension. A missing lower bound disables these
    /// checks when the caller has already scheduled the remaining range.
    fn commit_branches<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        next_path: &Nibbles,
        uncalculated_lower_bound: Option<&Nibbles>,
    ) -> Result<Option<(Nibbles, Option<Nibbles>)>, StateProofError> {
        let dirty_range = |prefix_set: &mut PrefixSet, upper_bound: Option<Nibbles>| {
            let uncalculated_lower_bound = uncalculated_lower_bound?;

            if upper_bound.as_ref().is_some_and(|upper| uncalculated_lower_bound >= upper) {
                return None
            }

            match upper_bound {
                Some(upper_bound) => prefix_set
                    .contains_range(uncalculated_lower_bound..&upper_bound)
                    .then_some((*uncalculated_lower_bound, Some(upper_bound))),
                None => prefix_set
                    .contains_from(uncalculated_lower_bound)
                    .then_some((*uncalculated_lower_bound, None)),
            }
        };

        let mut popped_child_path_upper = None;
        while !next_path.starts_with(&self.branch_path) {
            // If the lower bound is still within this branch, process any remaining dirty keys
            // before popping it so they can be added directly to the branch.
            if uncalculated_lower_bound.is_some_and(|lower| lower.starts_with(&self.branch_path)) &&
                let Some(range) =
                    dirty_range(&mut self.prefix_set, self.branch_path.next_without_prefix())
            {
                return Ok(Some(range))
            }

            let branch = self.branch_stack.last().expect("branch_stack cannot be empty");
            // Once popped, this branch becomes a child at this path. Its upper bound therefore
            // covers any keys which could split the branch's extension on the right.
            popped_child_path_upper = Some(
                self.branch_path
                    .slice_unchecked(0, self.branch_path.len() - branch.ext_len as usize)
                    .next_without_prefix(),
            );

            self.pop_branch(targets)?;
        }

        // An empty branch_stack is skipped because a popped local root does not need this check:
        // any gap before `next_path` was already returned by `try_pop_cached_branch`, and forward
        // traversal will split its extension and process later dirty keys as needed.
        if !self.branch_stack.is_empty() &&
            let Some(upper_bound) = popped_child_path_upper &&
            let Some(range) = dirty_range(&mut self.prefix_set, upper_bound)
        {
            return Ok(Some(range))
        }

        Ok(None)
    }

    // Returns the next child nibble on the current branch to process, or None if there are no
    // further nibbles to process.
    fn next_uncached_child_nibble(
        prefix_set: &mut PrefixSet,
        branch_path: &Nibbles,
        uncalculated_lower_bound_ref: &Nibbles,
        cached_state_mask: TrieMask,
    ) -> Option<u8> {
        let mut next_child_nibbles = cached_state_mask;

        // Also include child nibbles indicated by the prefix set. The prefix set can
        // indicate children that need recalculation from leaves (e.g. new keys inserted
        // under this branch).
        if prefix_set.contains(branch_path) {
            let branch_path_len = branch_path.len();
            let mut child_path = *branch_path;
            for nibble in 0u8..16 {
                child_path.truncate(branch_path_len);
                child_path.push_unchecked(nibble);
                if prefix_set.contains(&child_path) {
                    next_child_nibbles.set_bit(nibble);
                }
            }
        }

        let _orig_next_child_nibbles = next_child_nibbles;

        // Mask out any child nibbles whose ranges have already been fully processed.
        // This can happen when `calculate_key_range` finds no keys for a child's range,
        // leaving the child's bit unset in `state_mask`. Without this, re-entering this
        // function would select the same child again.
        if uncalculated_lower_bound_ref.starts_with(branch_path) &&
            uncalculated_lower_bound_ref.len() > branch_path.len()
        {
            let lower_nibble = uncalculated_lower_bound_ref.get_unchecked(branch_path.len());
            // Clear all nibbles strictly below `lower_nibble`. If the lower bound is within the
            // current child, the remainder of that child may still need to be processed.
            let already_processed_mask = TrieMask::new((1u16 << lower_nibble) - 1);
            next_child_nibbles &= !already_processed_mask;
            trace!(
                target: TRACE_TARGET,
                ?branch_path,
                ?_orig_next_child_nibbles,
                ?already_processed_mask,
                ?next_child_nibbles,
                "Unset already processed key nibbles from next_child_nibbles",
            );
        } else if !uncalculated_lower_bound_ref.starts_with(branch_path) &&
            uncalculated_lower_bound_ref > branch_path
        {
            // The lower bound has moved entirely past this branch (e.g. branch is 0x6 but
            // lower is 0x7). All remaining children have been processed.
            next_child_nibbles = TrieMask::default();
            trace!(
                target: TRACE_TARGET,
                ?branch_path,
                ?_orig_next_child_nibbles,
                ?next_child_nibbles,
                "Unset all nibbles from next_child_nibbles due to branch_path being outside this subtrie",
            );
        }

        next_child_nibbles.first_set_bit_index()
    }

    /// Accepts the current state of both hashed and trie cursors, and determines the next range of
    /// hashed keys which need to be processed using [`Self::calculate_key_range`].
    ///
    /// This method will use cached branch node data from the trie cursor to skip over all possible
    /// ranges of keys, to reduce computation as much as possible.
    ///
    /// # Returns
    ///
    /// - `None`: No more data to process, finish computation
    ///
    /// - `Some(lower, None)`: Indicates to process all keys starting at `lower`, with no upper
    ///   bound. This method won't be called again after this.
    ///
    /// - `Some(lower, Some(upper))`: Indicates to process all keys starting at `lower`, up to but
    ///   excluding `upper`, and then call this method once done.
    ///
    /// Once returned the `branch_stack` will be in the correct state to start calculating leaves
    /// for the given range, if any.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    fn next_uncached_key_range<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        trie_cursor_state: &mut TrieCursorState,
        traversal_upper_bound: Option<&Nibbles>,
        mut uncalculated_lower_bound: Option<Nibbles>,
    ) -> Result<Option<(Nibbles, Option<Nibbles>)>, StateProofError> {
        loop {
            if let (Some(lower_bound), Some(upper_bound)) =
                (uncalculated_lower_bound.as_ref(), traversal_upper_bound) &&
                lower_bound >= upper_bound
            {
                return Ok(None)
            }

            // Pop the currently cached branch node.
            //
            // NOTE we pop off the `cached_branch_stack` because cloning the `BranchNodeCompact`
            // means cloning an Arc, which incurs synchronization overhead. We have to be sure to
            // push the cached branch back onto the stack once done.
            let (cached_path, cached_branch) = match self.try_pop_cached_branch(
                trie_cursor_state,
                traversal_upper_bound,
                &uncalculated_lower_bound,
            )? {
                PopCachedBranchOutcome::Popped(cached) => cached,
                PopCachedBranchOutcome::Exhausted => {
                    // If cached branches are exhausted it's possible that there is still an
                    // unbounded range of leaves to be processed. `uncalculated_lower_bound` is
                    // used to return that range.
                    trace!(target: TRACE_TARGET, ?uncalculated_lower_bound, "Exhausted cached trie nodes");
                    if let Some(lower) = uncalculated_lower_bound {
                        self.commit_branches(targets, &lower, None)?;
                        return Ok(Some((lower, traversal_upper_bound.copied())));
                    }
                    return Ok(None)
                }
                PopCachedBranchOutcome::CalculateLeaves(range) => {
                    self.commit_branches(targets, &range.0, None)?;
                    return Ok(Some(range));
                }
            };

            let uncalculated_lower_bound_ref = uncalculated_lower_bound
                .as_ref()
                .expect("try_pop_cached_branch would return Exhausted if this were None");

            trace!(
                target: TRACE_TARGET,
                branch_path = ?self.branch_path,
                branch_state_mask = ?self.branch_stack.last().map(|b| b.state_mask),
                ?cached_path,
                cached_branch_state_mask = ?cached_branch.state_mask,
                cached_branch_hash_mask = ?cached_branch.hash_mask,
                "loop",
            );

            if let Some(range) =
                self.commit_branches(targets, &cached_path, Some(uncalculated_lower_bound_ref))?
            {
                self.cached_branch_stack.push((cached_path, cached_branch));
                return Ok(Some(range))
            }

            // Since we've popped all constructed branches which don't contain `cached_path`, the
            // remaining branch path must be its prefix.
            debug_assert!(
                self.branch_path.len() < cached_path.len() || self.branch_path == cached_path,
                "branch_path {:?} is different-or-longer-than cached_path {cached_path:?}",
                self.branch_path
            );

            // Dirty keys before this cached path may split the extension leading to it. Process
            // them before using the cached branch as a hint.
            if uncalculated_lower_bound_ref < &cached_path &&
                self.prefix_set.contains_range(uncalculated_lower_bound_ref..&cached_path)
            {
                self.cached_branch_stack.push((cached_path, cached_branch));
                return Ok(Some((*uncalculated_lower_bound_ref, Some(cached_path))))
            }

            let child_nibble = Self::next_uncached_child_nibble(
                &mut self.prefix_set,
                &cached_path,
                uncalculated_lower_bound_ref,
                cached_branch.state_mask,
            );

            let Some(child_nibble) = child_nibble else {
                trace!(
                    target: TRACE_TARGET,
                    path=?cached_path,
                    ?cached_branch,
                    "No further cached children",
                );

                // no need to pop from `cached_branch_stack`, the current cached branch is already
                // popped (see note at the top of the loop).

                // The completed branch has no more keys with its prefix. Set the lower bound which
                // can be returned from this method to be the next possible prefix, if any.
                uncalculated_lower_bound = cached_path.next_without_prefix();

                continue
            };

            let mut child_path = cached_path;
            child_path.push_unchecked(child_nibble);
            let child_lower_bound = (*uncalculated_lower_bound_ref).max(child_path);

            // If the `hash_mask` bit is set for the next child it means the child's hash is cached
            // in the `cached_branch`. We can use that instead of re-calculating the hash of the
            // entire sub-trie.
            //
            // If the child needs to be retained for a proof then we should not use the cached
            // hash, and instead continue on to calculate its node manually.
            //
            // If the child's path is in the prefix set then the cached hash is stale and must
            // not be used.
            if cached_branch.hash_mask.is_bit_set(child_nibble) &&
                child_lower_bound == child_path &&
                !self.prefix_set.contains(&child_path)
            {
                // Pull this child's hash out of the cached branch node. The hash index is the
                // number of hash_mask bits set below this child's nibble.
                let lower_bits = TrieMask::new((1u16 << child_nibble) - 1);
                let hash_idx = (cached_branch.hash_mask & lower_bits).count_ones() as usize;
                let hash = cached_branch.hashes[hash_idx];

                // `take_cached_branch` replaces hashes with zero when their nodes must be
                // revealed to support a possible branch collapse.
                if hash != B256::ZERO {
                    let mut probed_targets = targets.clone();
                    if !self.should_retain(&mut probed_targets, &child_path, false) {
                        trace!(
                            target: TRACE_TARGET,
                            ?child_path,
                            ?hash_idx,
                            ?hash,
                            "Using cached hash for child",
                        );

                        // Keep this hint available while inserting the child so a branch which is
                        // naturally materialized at `cached_path` can inherit its masks.
                        let tree_mask_bit = cached_branch.tree_mask.is_bit_set(child_nibble);
                        self.cached_branch_stack.push((cached_path, cached_branch));
                        self.push_child(
                            targets,
                            ProofTrieBranchChild::RlpNode {
                                node: RlpNode::word_rlp(&hash),
                                short_key: child_path,
                                hash_mask_bit: true,
                                tree_mask_bit,
                            },
                        )?;

                        if let (Some(targets), Some(probed_targets)) =
                            (targets.as_mut(), probed_targets)
                        {
                            targets.i = targets.i.max(probed_targets.i);
                        }

                        // Update the `uncalculated_lower_bound` to indicate that the child whose
                        // bit was just set is completely processed.
                        uncalculated_lower_bound = child_path.next_without_prefix();

                        continue
                    }
                }
            }

            // We now want to check if there is a cached branch node at this child. The cached
            // branch node may be the node at this child directly, or this child may be an
            // extension and the cached branch is the child of that extension.

            // All trie nodes prior to `child_lower_bound` have been processed, so seek the trie
            // cursor forward if necessary.
            if trie_cursor_state.path().is_some_and(|path| path < &child_lower_bound) {
                trace!(target: TRACE_TARGET, ?child_lower_bound, "Seeking trie cursor to child lower bound");
                *trie_cursor_state = TrieCursorState::seeked(
                    child_lower_bound,
                    self.trie_cursor.seek(child_lower_bound)?,
                );
            }

            // If the next cached branch node is a child of `child_path` then we can assume it is
            // the cached branch for this child. We push it onto the `cached_branch_stack` and loop
            // back to the top.
            if let TrieCursorState::Available(next_cached_path, next_cached_branch) =
                &trie_cursor_state &&
                next_cached_path.starts_with(&child_path)
            {
                // Push the current cached branch back on before pushing its child and then looping
                self.cached_branch_stack.push((cached_path, cached_branch));

                trace!(
                    target: TRACE_TARGET,
                    ?child_path,
                    ?next_cached_path,
                    ?next_cached_branch,
                    "Pushing cached branch for child",
                );
                let cached = self.take_cached_branch(trie_cursor_state);
                self.cached_branch_stack.push(cached);
                continue;
            }

            // There is no cached data for the sub-trie at this child, we must recalculate the
            // sub-trie root (this child) using the leaves. Return the range of keys based on the
            // child path.
            let child_upper_bound = child_path.next_without_prefix();
            trace!(
                target: TRACE_TARGET,
                lower=?child_lower_bound,
                upper=?child_upper_bound,
                "Returning sub-trie's key range to calculate",
            );

            // Push the current cached branch back onto the stack before returning.
            self.cached_branch_stack.push((cached_path, cached_branch));

            return Ok(Some((child_lower_bound, child_upper_bound)));
        }
    }

    /// Calculates trie nodes and retains proofs for targeted nodes within a sub-trie. The
    /// sub-trie's bounds are denoted by the `lower_bound` and `upper_bound` arguments,
    /// `upper_bound` is exclusive, None indicates unbounded.
    #[instrument(
        target = TRACE_TARGET,
        level = "trace",
        skip_all,
        fields(
            parent_prefix=?sub_trie_targets.parent_prefix,
            lower_bound=?sub_trie_targets.lower_bound,
            upper_bound=?sub_trie_targets.upper_bound,
        ),
    )]
    fn proof_subtrie<'a>(
        &mut self,
        value_encoder: &mut VE,
        trie_cursor_state: &mut TrieCursorState,
        hashed_cursor_state: &mut HashedCursorState<VE::DeferredEncoder>,
        sub_trie_targets: SubTrieTargets<'a>,
    ) -> Result<(), StateProofError> {
        let traversal_lower_bound = sub_trie_targets.lower_bound;
        let traversal_upper_bound = sub_trie_targets.upper_bound;

        // Wrap targets into a `TargetsCursor`.  targets can be empty if we only want to calculate
        // the root, in which case we don't need a cursor.
        let mut targets = if sub_trie_targets.targets.is_empty() {
            None
        } else {
            Some(TargetsCursor::new(sub_trie_targets.targets))
        };

        // Ensure initial state is cleared. By the end of the method call these should be empty once
        // again.
        debug_assert!(self.cached_branch_stack.is_empty());
        debug_assert!(self.branch_stack.is_empty());
        debug_assert!(self.branch_path.is_empty());
        debug_assert!(self.child_stack.is_empty());

        // `next_uncached_key_range`, which will be called in the loop below, expects the trie
        // cursor to have already been positioned. Cursor resets for overlapping sub-tries are
        // handled by `proof_inner`, so a buffered entry at-or-after this disjoint range remains the
        // first unconsumed entry. Exhaustion is similarly stable across forward-only ranges.
        if trie_cursor_state.needs_seek_to(&traversal_lower_bound) {
            trace!(target: TRACE_TARGET, "Doing initial seek of trie cursor");
            *trie_cursor_state = TrieCursorState::seeked(
                traversal_lower_bound,
                self.trie_cursor.seek(traversal_lower_bound)?,
            );
        }

        // `uncalculated_lower_bound` tracks the lower bound of node paths which have yet to be
        // visited, either via the hashed key cursor (`calculate_key_range`) or trie cursor
        // (`next_uncached_key_range`). If/when this becomes None then there are no further nodes
        // which could exist.
        let mut uncalculated_lower_bound = Some(traversal_lower_bound);

        trace!(target: TRACE_TARGET, "Starting loop");
        loop {
            // Save the previous lower bound to detect forward progress.
            let prev_uncalculated_lower_bound = uncalculated_lower_bound;

            // Determine the range of keys of the overall trie which need to be re-computed.
            let Some((calc_lower_bound, calc_upper_bound)) = self.next_uncached_key_range(
                &mut targets,
                trie_cursor_state,
                traversal_upper_bound.as_ref(),
                prev_uncalculated_lower_bound,
            )?
            else {
                // If `next_uncached_key_range` determines that there can be no more keys then
                // complete the computation.
                break;
            };

            // Forward-progress guard: detect trie inconsistencies that would cause infinite loops.
            // If `next_uncached_key_range` returns a range that starts before the previous
            // lower bound, we've gone backwards and would loop forever.
            //
            // This can specifically happen when there is a cached branch which shouldn't exist, or
            // if state mask bit is set on a cached branch which shouldn't be.
            if let Some(prev_lower) = prev_uncalculated_lower_bound.as_ref() &&
                calc_lower_bound < *prev_lower
            {
                let msg = format!(
                    "next_uncached_key_range went backwards: calc_lower={calc_lower_bound:?} < \
                     prev_lower={prev_lower:?}, calc_upper={calc_upper_bound:?}, \
                     lower_bound={traversal_lower_bound:?}, \
                     upper_bound={traversal_upper_bound:?}",
                );
                error!(target: TRACE_TARGET, "{msg}");
                return Err(StateProofError::TrieInconsistency(msg));
            }

            // Calculate the trie for that range of keys
            self.calculate_key_range(
                value_encoder,
                &mut targets,
                hashed_cursor_state,
                calc_lower_bound,
                calc_upper_bound,
            )?;

            // Once outside `calculate_key_range`, `hashed_cursor_state` will be at the first key
            // after the range, or exhausted.
            //
            // If the hashed cursor is exhausted, or has reached the end of the traversal range,
            // then there are no more keys which can contribute to these target children.
            if hashed_cursor_state.path().is_none_or(|key| {
                traversal_upper_bound.is_some_and(|upper_bound| key >= &upper_bound)
            }) {
                break;
            }

            // The upper bound of previous calculation becomes the lower bound of the uncalculated
            // range, for which we'll once again check for cached data.
            uncalculated_lower_bound = calc_upper_bound;
        }

        // Once there's no more leaves we can pop the remaining branches, if any.
        trace!(target: TRACE_TARGET, "Exited loop, popping remaining branches");
        while !self.branch_stack.is_empty() {
            self.pop_branch(&mut targets)?;
        }

        // At this point the branch stack should be empty. If the child stack is empty it means no
        // keys were ever iterated from the hashed cursor in the first place. Otherwise there should
        // only be a single node left: the root node.
        debug_assert!(self.branch_stack.is_empty());
        debug_assert!(self.branch_path.is_empty());
        debug_assert!(self.child_stack.len() < 2);

        // The `cached_branch_stack` may still have cached branches on it, as it's not affected by
        // `pop_branch`, but it is no longer needed and should be cleared.
        self.cached_branch_stack.clear();

        // We always pop the local root node off of the `child_stack` in order to empty it. If the
        // parent branch is already known, compressed roots need to be rebased into a direct child
        // of that parent before they can be attached to it.
        trace!(
            target: TRACE_TARGET,
            parent_prefix = ?sub_trie_targets.parent_prefix,
            child_stack_empty = self.child_stack.is_empty(),
            "Maybe retaining local root",
        );
        let root_node = self.child_stack.pop();

        // A full-trie calculation always retains a root, using an empty root when traversal
        // produced no root node.
        let Some(parent_prefix) = sub_trie_targets.parent_prefix else {
            let mut root_node = if let Some(root_node) = root_node {
                self.rlp_encode_buf.clear();
                root_node.into_proof_trie_node(Nibbles::new(), &mut self.rlp_encode_buf)?
            } else {
                ProofTrieNodeV2::empty()
            };

            // Direct root branches do not have an entry in the branch node table. A root extension
            // still carries the masks of the child branch embedded within it.
            if matches!(&root_node.node, TrieNodeV2::Branch(branch) if branch.key.is_empty()) {
                root_node.masks = None;
            }

            self.retained_proofs.push(root_node);
            return Ok(())
        };

        // If there's no root node then the subtrie has no keys, return nothing.
        let Some(mut root_node) = root_node else { return Ok(()) };

        let root_full_path = *root_node.short_key();

        // An exact match reconstructed the already-revealed parent; its targeted children were
        // retained while that parent branch was popped.
        if root_full_path == parent_prefix {
            return Ok(())
        }

        // At this point we have a "root" node which the calculator has based at 0x (empty path),
        // but the subtrie targets indicate that there is a known parent branch at parent_prefix
        // which this root should be rebased onto.

        // The local root of a partial calculation must be at or below its known parent.
        if !root_full_path.starts_with(&parent_prefix) {
            return Err(StateProofError::TrieInconsistency(format!(
                "local root path {root_full_path:?} does not start with parent prefix \
                 {parent_prefix:?}",
            )))
        }

        // Keep the parent branch's child nibble in the proof path so the local root attaches
        // directly below that parent.
        let child_path_len = parent_prefix.len() + 1;
        let child_path = root_full_path.slice_unchecked(0, child_path_len);

        // It's possible that the local root lies on a child which is not targeted.
        if !sub_trie_targets
            .targets
            .iter()
            .any(|target| target.key_nibbles.starts_with(&child_path))
        {
            return Ok(())
        }

        // Retain the requested child with only the path below its parent edge in the short key.
        root_node.trim_short_key_prefix(child_path_len);
        self.rlp_encode_buf.clear();
        let root_node = root_node.into_proof_trie_node(child_path, &mut self.rlp_encode_buf)?;
        self.retained_proofs.push(root_node);

        Ok(())
    }

    /// Clears internal computation state. Called after errors to ensure the calculator is not
    /// left in a partially-computed state when reused.
    fn clear_computation_state(&mut self) {
        self.branch_stack.clear();
        self.branch_path = Nibbles::new();
        self.child_stack.clear();
        self.cached_branch_stack.clear();
        self.retained_proofs.clear();
    }

    /// Internal implementation of proof calculation. Assumes both cursors have already been reset.
    /// See docs on [`Self::proof`] for expected behavior.
    fn proof_inner(
        &mut self,
        value_encoder: &mut VE,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        // If there are no targets then nothing could be returned, return early.
        if targets.is_empty() {
            trace!(target: TRACE_TARGET, "Empty targets, returning");
            return Ok(Vec::new())
        }

        // Initialize the variables which track the state of the two cursors. Both indicate the
        // cursors are unseeked.
        let mut trie_cursor_state = TrieCursorState::unseeked();
        let mut hashed_cursor_state = HashedCursorState::unseeked();
        let mut previous_traversal_bounds: Option<(Nibbles, Option<Nibbles>)> = None;

        // Divide targets into bounded ranges, each corresponding to the direct children of one
        // already-revealed parent, and handle all proofs within that range.
        for sub_trie_targets in iter_sub_trie_targets(targets) {
            let traversal_lower_bound = sub_trie_targets.lower_bound;
            let traversal_upper_bound = sub_trie_targets.upper_bound;
            if previous_traversal_bounds.is_some_and(|(_, previous_upper_bound)| {
                previous_upper_bound.is_none_or(|upper_bound| upper_bound > traversal_lower_bound)
            }) {
                if trie_cursor_state.needs_reset_before_seek(&traversal_lower_bound) {
                    trace!(
                        target: TRACE_TARGET,
                        ?previous_traversal_bounds,
                        ?traversal_lower_bound,
                        ?traversal_upper_bound,
                        "Resetting trie cursor before overlapping or backward traversal range",
                    );
                    self.trie_cursor.reset();
                    trie_cursor_state = TrieCursorState::unseeked();
                }
                if hashed_cursor_state.needs_reset_before_seek(&traversal_lower_bound) {
                    trace!(
                        target: TRACE_TARGET,
                        ?previous_traversal_bounds,
                        ?traversal_lower_bound,
                        ?traversal_upper_bound,
                        "Resetting hashed cursor before overlapping or backward traversal range",
                    );
                    self.hashed_cursor.reset();
                    hashed_cursor_state = HashedCursorState::unseeked();
                }
            }

            if let Err(err) = self.proof_subtrie(
                value_encoder,
                &mut trie_cursor_state,
                &mut hashed_cursor_state,
                sub_trie_targets,
            ) {
                self.clear_computation_state();
                return Err(err);
            }

            previous_traversal_bounds = Some((traversal_lower_bound, traversal_upper_bound));
        }

        trace!(
            target: TRACE_TARGET,
            retained_proofs_len = ?self.retained_proofs.len(),
            "proof_inner: returning",
        );
        self.retained_proofs.sort_unstable_by(|a, b| depth_first::cmp(&a.path, &b.path));
        self.retained_proofs.dedup_by(|a, b| a.path == b.path);
        Ok(core::mem::take(&mut self.retained_proofs))
    }

    /// Generate a proof for the given targets.
    ///
    /// Given a set of [`ProofV2Target`]s, returns nodes whose paths are a prefix of any target. The
    /// returned nodes will be sorted depth-first by path.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    pub fn proof(
        &mut self,
        value_encoder: &mut VE,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        self.trie_cursor.reset();
        self.hashed_cursor.reset();
        self.proof_inner(value_encoder, targets)
    }

    /// Computes the root hash from a set of proof nodes.
    ///
    /// Returns `None` if there is no root node (partial proof), otherwise returns the hash of the
    /// root node.
    ///
    /// This method reuses the internal RLP encode buffer for efficiency.
    pub fn compute_root_hash(
        &mut self,
        proof_nodes: &[ProofTrieNodeV2],
    ) -> Result<Option<B256>, StateProofError> {
        // Find the root node (node at empty path)
        let root_node = proof_nodes.iter().find(|node| node.path.is_empty());

        let Some(root) = root_node else {
            return Ok(None);
        };

        // Compute the hash of the root node
        self.rlp_encode_buf.clear();
        root.node.encode(&mut self.rlp_encode_buf);
        let root_hash = keccak256(&self.rlp_encode_buf);

        Ok(Some(root_hash))
    }

    /// Calculates the root node of the trie.
    ///
    /// This method does not accept targets nor retain proofs. Returns the root node which can
    /// be used to compute the root hash via [`Self::compute_root_hash`].
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self, value_encoder))]
    pub fn root_node(
        &mut self,
        value_encoder: &mut VE,
    ) -> Result<ProofTrieNodeV2, StateProofError> {
        self.trie_cursor.reset();
        self.hashed_cursor.reset();

        // Initialize the variables which track the state of the two cursors. Both indicate the
        // cursors are unseeked.
        let mut trie_cursor_state = TrieCursorState::unseeked();
        let mut hashed_cursor_state = HashedCursorState::unseeked();

        static EMPTY_TARGETS: [ProofV2Target; 0] = [];
        let sub_trie_targets = SubTrieTargets {
            lower_bound: Nibbles::new(),
            upper_bound: None,
            parent_prefix: None,
            targets: &EMPTY_TARGETS,
        };

        if let Err(err) = self.proof_subtrie(
            value_encoder,
            &mut trie_cursor_state,
            &mut hashed_cursor_state,
            sub_trie_targets,
        ) {
            self.clear_computation_state();
            return Err(err);
        }

        // `proof_subtrie` retains the root node when there is no known parent, regardless of
        // whether there are any targets.
        let mut proofs = core::mem::take(&mut self.retained_proofs);
        trace!(
            target: TRACE_TARGET,
            proofs_len = ?proofs.len(),
            "root_node: extracting root",
        );

        // The root node is at the empty path. Since there is no parent and targets is empty, there
        // should be no other retained proofs.
        debug_assert_eq!(
            proofs.len(), 1,
            "prefix is empty, parent path is None, and targets is empty, so there must be only the root node"
        );

        // Find and remove the root node (node at empty path)
        let root_node = proofs.pop().expect("prefix is empty, parent path is None, and targets is empty, so there must be only the root node");

        Ok(root_node)
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
    pub fn new_storage(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self::new(trie_cursor, hashed_cursor)
    }

    /// Generate a proof for a storage trie at the given hashed address.
    ///
    /// Given a set of [`ProofV2Target`]s, returns nodes whose paths are a prefix of any target. The
    /// returned nodes will be sorted depth-first by path.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self, targets))]
    pub fn storage_proof(
        &mut self,
        hashed_address: B256,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        // Shortcut: check if storage is empty
        if self.hashed_cursor.is_storage_empty()? {
            return Ok(if targets.iter().any(|target| !target.parent.is_known()) {
                vec![ProofTrieNodeV2 {
                    path: Nibbles::default(),
                    node: TrieNodeV2::EmptyRoot,
                    masks: None,
                }]
            } else {
                Vec::new()
            })
        }

        // Don't call `set_hashed_address` on the trie cursor until after the previous shortcut has
        // been checked.
        self.trie_cursor.set_hashed_address(hashed_address);

        // Create a mutable storage value encoder
        let mut storage_value_encoder = StorageValueEncoder;
        self.proof_inner(&mut storage_value_encoder, targets)
    }

    /// Calculates the root node of a storage trie.
    ///
    /// This method does not accept targets nor retain proofs. Returns the root node which can
    /// be used to compute the root hash via [`Self::compute_root_hash`].
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self))]
    pub fn storage_root_node(
        &mut self,
        hashed_address: B256,
    ) -> Result<ProofTrieNodeV2, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        if self.hashed_cursor.is_storage_empty()? {
            return Ok(ProofTrieNodeV2 {
                path: Nibbles::default(),
                node: TrieNodeV2::EmptyRoot,
                masks: None,
            })
        }

        // Don't call `set_hashed_address` on the trie cursor until after the previous shortcut has
        // been checked.
        self.trie_cursor.set_hashed_address(hashed_address);

        // Create a mutable storage value encoder
        let mut storage_value_encoder = StorageValueEncoder;
        self.root_node(&mut storage_value_encoder)
    }
}

/// Helper type wrapping a slice of [`ProofV2Target`]s, primarily used to iterate through targets in
/// [`ProofCalculator::should_retain`].
///
/// It is assumed that the underlying slice is never empty, and that the iterator is never
/// exhausted.
#[derive(Clone)]
struct TargetsCursor<'a> {
    targets: &'a [ProofV2Target],
    i: usize,
}

impl<'a> TargetsCursor<'a> {
    /// Wraps a slice of [`ProofV2Target`]s with the `TargetsCursor`.
    ///
    /// # Panics
    ///
    /// Will panic in debug mode if called with an empty slice.
    fn new(targets: &'a [ProofV2Target]) -> Self {
        debug_assert!(!targets.is_empty());
        Self { targets, i: 0 }
    }

    /// Returns the current and next [`ProofV2Target`] that the cursor is pointed at.
    fn current(&self) -> (&'a ProofV2Target, Option<&'a ProofV2Target>) {
        (&self.targets[self.i], self.targets.get(self.i + 1))
    }

    /// Iterates the cursor forward.
    ///
    /// # Panics
    ///
    /// Will panic if the cursor is exhausted.
    fn next(&mut self) -> (&'a ProofV2Target, Option<&'a ProofV2Target>) {
        self.i += 1;
        debug_assert!(self.i < self.targets.len());
        self.current()
    }

    // Iterate forwards over the slice, starting from the [`ProofV2Target`] after the current.
    fn skip_iter(&self) -> impl Iterator<Item = &'a ProofV2Target> {
        self.targets[self.i + 1..].iter()
    }

    /// Iterated backwards over the slice, starting from the [`ProofV2Target`] previous to the
    /// current.
    fn rev_iter(&self) -> impl Iterator<Item = &'a ProofV2Target> {
        self.targets[..self.i].iter().rev()
    }
}

/// Used to track the state of the trie cursor, allowing us to differentiate between a branch having
/// been taken (used as a cached branch) and the cursor having been exhausted.
#[derive(Debug)]
enum TrieCursorState {
    /// The initial state of the cursor, indicating it's never been seeked.
    Unseeked,
    /// Cursor is seeked to this path and the node has not been used yet.
    Available(Nibbles, BranchNodeCompact),
    /// Cursor is seeked to this path, but the node has been used.
    Taken(Nibbles),
    /// Cursor has been exhausted after seeking from the given lower bound.
    Exhausted(Nibbles),
}

impl TrieCursorState {
    /// Creates a [`Self::Unseeked`] based on an entry returned from the cursor itself.
    const fn unseeked() -> Self {
        Self::Unseeked
    }

    /// Creates a [`Self`] based on an entry returned from the cursor itself.
    fn seeked(key: Nibbles, entry: Option<(Nibbles, BranchNodeCompact)>) -> Self {
        entry.map_or(Self::Exhausted(key), |(path, node)| Self::Available(path, node))
    }

    /// Returns the path the cursor is seeked to, or None if it's exhausted.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is unseeked.
    const fn path(&self) -> Option<&Nibbles> {
        match self {
            Self::Unseeked => panic!("cursor is unseeked"),
            Self::Available(path, _) | Self::Taken(path) => Some(path),
            Self::Exhausted(_) => None,
        }
    }

    /// Returns true if the cursor must seek to be usable for a range starting at `path`.
    fn needs_seek_to(&self, path: &Nibbles) -> bool {
        match self {
            Self::Unseeked | Self::Taken(_) => true,
            Self::Available(current_path, _) => current_path < path,
            Self::Exhausted(_) => false,
        }
    }

    /// Returns true if seeking to `key` requires resetting the forward-only cursor.
    fn needs_reset_before_seek(&self, key: &Nibbles) -> bool {
        match self {
            Self::Unseeked => false,
            Self::Available(path, _) | Self::Taken(path) => path > key,
            Self::Exhausted(exhausted_at) => exhausted_at > key,
        }
    }

    /// Takes the path and node from a [`Self::Available`]. Panics if not [`Self::Available`].
    fn take(&mut self) -> (Nibbles, BranchNodeCompact) {
        let Self::Available(path, _) = self else {
            panic!("take called on non-Available: {self:?}")
        };

        let path = *path;
        let Self::Available(path, node) = core::mem::replace(self, Self::Taken(path)) else {
            unreachable!("already checked that self is Self::Available");
        };

        (path, node)
    }
}

/// Used to track the state of the hashed cursor, including the path that established exhaustion.
enum HashedCursorState<V> {
    /// The initial state of the cursor, indicating it's never been seeked.
    Unseeked,
    /// Cursor is seeked to this path and the value has not been used yet.
    Available(Nibbles, V),
    /// Cursor has been exhausted at or after the given path.
    Exhausted(Nibbles),
}

impl<V> HashedCursorState<V> {
    /// Creates a [`Self::Unseeked`] state.
    const fn unseeked() -> Self {
        Self::Unseeked
    }

    /// Creates a [`Self`] based on an entry returned from the cursor itself.
    fn seeked(key: Nibbles, entry: Option<(Nibbles, V)>) -> Self {
        entry.map_or(Self::Exhausted(key), |(path, value)| Self::Available(path, value))
    }

    /// Returns the path the cursor is seeked to, or None if it's unseeked or exhausted.
    const fn path(&self) -> Option<&Nibbles> {
        match self {
            Self::Available(path, _) => Some(path),
            Self::Unseeked | Self::Exhausted(_) => None,
        }
    }

    /// Returns true if the cursor must seek to be usable for a range starting at `key`.
    fn needs_seek_to(&self, key: &Nibbles) -> bool {
        match self {
            Self::Unseeked => true,
            Self::Available(path, _) => path < key,
            Self::Exhausted(exhausted_at) => exhausted_at > key,
        }
    }

    /// Returns true if seeking to `key` requires resetting the forward-only cursor.
    fn needs_reset_before_seek(&self, key: &Nibbles) -> bool {
        match self {
            Self::Unseeked => false,
            Self::Available(path, _) => path > key,
            Self::Exhausted(exhausted_at) => exhausted_at >= key,
        }
    }

    /// Takes the path and value from a [`Self::Available`]. Panics if not [`Self::Available`].
    fn take(&mut self) -> (Nibbles, V) {
        match core::mem::replace(self, Self::Unseeked) {
            Self::Available(path, value) => (path, value),
            _ => panic!("take called on non-Available hashed cursor state"),
        }
    }
}

/// Describes the state of the currently cached branch node (if any).
enum PopCachedBranchOutcome {
    /// Cached branch has been popped from the `cached_branch_stack` and is ready to be used.
    Popped((Nibbles, BranchNodeCompact)),
    /// All cached branches have been exhausted.
    Exhausted,
    /// Need to calculate leaves from this range (exclusive upper) before the cached branch
    /// (catch-up range). If None then
    CalculateLeaves((Nibbles, Option<Nibbles>)),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{
            mock::MockHashedCursorFactory, noop::NoopHashedCursor, HashedCursorFactory,
            HashedPostStateCursor,
        },
        proof::StorageProof as LegacyStorageProof,
        test_utils::TrieTestHarness,
        trie_cursor::{
            depth_first, noop::NoopStorageTrieCursor, InMemoryTrieCursor, TrieCursorFactory,
        },
    };
    use alloy_primitives::map::B256Set;
    use alloy_rlp::Decodable;
    use alloy_trie::proof::AddedRemovedKeys;
    use itertools::Itertools;
    use reth_trie_common::{
        prefix_set::{PrefixSet, PrefixSetMut},
        updates::TrieUpdatesSorted,
        HashedPostState, HashedStorage, ProofTrieNode, ProofV2TargetParent, TrieNode,
        EMPTY_ROOT_HASH,
    };
    use std::collections::BTreeMap;

    /// Converts legacy proofs to V2 proofs by combining extension nodes with their child branch
    /// nodes.
    ///
    /// In the legacy proof format, extension nodes and branch nodes are separate. In the V2 format,
    /// they are combined into a single `BranchNodeV2` where the extension's key becomes the
    /// branch's `key` field.
    ///
    /// Converts legacy proofs (sorted in depth-first order) to V2 format.
    ///
    /// In depth-first order, children come BEFORE parents. So when we encounter an extension node,
    /// its child branch has already been processed and is in the result. We need to pop it and
    /// combine it with the extension.
    fn convert_legacy_proofs_to_v2(legacy_proofs: &[ProofTrieNode]) -> Vec<ProofTrieNodeV2> {
        ProofTrieNodeV2::from_sorted_trie_nodes(
            legacy_proofs.iter().map(|p| (p.path, p.node.clone(), p.masks)),
        )
    }

    /// Projects a legacy proof node into the representation requested by a V2 target.
    fn project_legacy_proof_node(
        node: &ProofTrieNodeV2,
        target: &ProofV2Target,
    ) -> Option<ProofTrieNodeV2> {
        let Some(parent_path_len) = target.parent.path_len() else {
            return target.key_nibbles.starts_with(&node.path).then(|| node.clone())
        };

        if node.path.len() > parent_path_len {
            return target.key_nibbles.starts_with(&node.path).then(|| node.clone())
        }

        let logical_path = match &node.node {
            TrieNodeV2::Leaf(leaf) => node.path.join(&leaf.key),
            TrieNodeV2::Branch(branch) => node.path.join(&branch.key),
            TrieNodeV2::EmptyRoot | TrieNodeV2::Extension(_) => return None,
        };
        let child_path_len = parent_path_len + 1;
        if logical_path.len() < child_path_len {
            return None
        }

        let child_path = logical_path.slice(0..child_path_len);
        if !target.key_nibbles.starts_with(&child_path) {
            return None
        }

        let trim_len = child_path_len - node.path.len();
        let mut projected = node.clone();
        projected.path = child_path;
        match &mut projected.node {
            TrieNodeV2::Leaf(leaf) => leaf.key = leaf.key.slice(trim_len..),
            TrieNodeV2::Branch(branch) => {
                branch.key = branch.key.slice(trim_len..);
                if branch.key.is_empty() {
                    branch.branch_rlp_node = None;
                }
            }
            TrieNodeV2::EmptyRoot | TrieNodeV2::Extension(_) => unreachable!(),
        }
        Some(projected)
    }

    /// Builds the exact V2 representation expected for a legacy proof and set of targets.
    fn project_legacy_proof(
        legacy_nodes: &[ProofTrieNodeV2],
        targets: &[ProofV2Target],
    ) -> Vec<ProofTrieNodeV2> {
        let mut projected = targets
            .iter()
            .flat_map(|target| {
                legacy_nodes.iter().filter_map(move |node| project_legacy_proof_node(node, target))
            })
            .collect::<Vec<_>>();
        projected.sort_unstable_by(|a, b| depth_first::cmp(&a.path, &b.path));
        projected.dedup_by(|a, b| {
            if a.path != b.path {
                return false
            }
            assert_eq!(a, b, "target projections disagree at path {:?}", a.path);
            true
        });
        projected
    }

    /// A test harness for comparing `StorageProofCalculator` and legacy `StorageProof`
    /// implementations.
    ///
    /// Wraps [`TrieTestHarness`] and adds a method to test that both proof implementations
    /// produce equivalent results for storage proofs.
    struct ProofTestHarness {
        inner: TrieTestHarness,
    }

    impl std::ops::Deref for ProofTestHarness {
        type Target = TrieTestHarness;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl ProofTestHarness {
        /// Creates a new test harness from a map of hashed storage slots to values.
        fn new(storage: BTreeMap<B256, U256>) -> Self {
            Self { inner: TrieTestHarness::new(storage) }
        }

        /// Computes the storage root while treating the supplied prefixes as dirty.
        fn root_with_prefix_set(&self, prefix_set: PrefixSet) -> Option<B256> {
            let trie_cursor =
                self.trie_cursor_factory().storage_trie_cursor(self.hashed_address()).unwrap();
            let hashed_cursor =
                self.hashed_cursor_factory().hashed_storage_cursor(self.hashed_address()).unwrap();
            let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
                .with_prefix_set(prefix_set);

            let mut targets = [ProofV2Target::new(B256::ZERO)];
            let proof = calculator.storage_proof(self.hashed_address(), &mut targets).unwrap();
            calculator.compute_root_hash(&proof).unwrap()
        }

        /// Asserts that `StorageProofCalculator` and legacy `StorageProof` produce equivalent
        /// results for storage proofs.
        fn assert_proof(
            &self,
            targets: impl IntoIterator<Item = ProofV2Target>,
        ) -> Result<(), StateProofError> {
            let mut targets_vec = targets.into_iter().collect::<Vec<_>>();

            // Get v2 proof and root hash via harness
            let (proof_v2_result, root_hash) = self.proof_v2(&mut targets_vec);

            // Verify the root hash matches the expected root (if the proof contains a root
            // node)
            if let Some(root_hash) = root_hash {
                pretty_assertions::assert_eq!(self.original_root(), root_hash);
            }

            // Fully materialize the legacy proof so compressed branches can be projected across
            // arbitrary parent boundaries, including absence targets that diverge inside them.
            let legacy_targets = targets_vec
                .iter()
                .map(|target| B256::from_slice(&target.key_nibbles.pack()))
                .chain(self.storage().keys().copied())
                .collect::<B256Set>();

            // Call legacy StorageProof::storage_multiproof
            let proof_legacy_result = LegacyStorageProof::new_hashed(
                self.trie_cursor_factory(),
                self.hashed_cursor_factory(),
                self.hashed_address(),
            )
            .with_branch_node_masks(true)
            .with_added_removed_keys(Some(AddedRemovedKeys::default().with_assume_added(true)))
            .storage_multiproof(legacy_targets)?;

            // Decode and sort legacy proof nodes
            let proof_legacy_nodes = proof_legacy_result
                .subtree
                .iter()
                .map(|(path, node_enc)| {
                    let mut buf = node_enc.as_ref();
                    let node = TrieNode::decode(&mut buf)
                        .expect("legacy implementation should not produce malformed proof nodes");

                    let masks = if path.is_empty() {
                        None
                    } else {
                        proof_legacy_result.branch_node_masks.get(path).copied()
                    };

                    ProofTrieNode { path: *path, node, masks }
                })
                .sorted_by(|a, b| depth_first::cmp(&a.path, &b.path))
                .collect::<Vec<_>>();

            // Convert legacy proofs to V2 proofs by combining extensions with their child branches
            let all_legacy_nodes_v2 = convert_legacy_proofs_to_v2(&proof_legacy_nodes);

            let expected_v2 = project_legacy_proof(&all_legacy_nodes_v2, &targets_vec);
            pretty_assertions::assert_eq!(expected_v2, proof_v2_result);

            Ok(())
        }
    }

    /// Tests that `clear_computation_state` properly resets internal stacks, allowing a
    /// `StorageProofCalculator` to be reused after a mid-computation error left stale state.
    /// Before the fix, stale data in `branch_stack`, `child_stack`, and `branch_path`
    /// could cause a `usize` underflow panic in `pop_branch`.
    #[test]
    fn test_proof_calculator_reuse_after_error() {
        reth_tracing::init_test_tracing();

        let slots = [
            B256::right_padding_from(&[0x10]),
            B256::right_padding_from(&[0x20]),
            B256::right_padding_from(&[0x30]),
            B256::right_padding_from(&[0x40]),
        ];
        let storage: BTreeMap<B256, U256> =
            slots.iter().map(|&s| (s, U256::from(100u64))).collect();

        let harness = ProofTestHarness::new(storage);

        let trie_cursor_factory = harness.trie_cursor_factory();
        let hashed_cursor_factory = harness.hashed_cursor_factory();

        let hashed_address = harness.hashed_address();
        let trie_cursor = trie_cursor_factory.storage_trie_cursor(hashed_address).unwrap();
        let hashed_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address).unwrap();
        let mut proof_calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        // Simulate stale state left by a mid-computation error: push fake entries onto internal
        // stacks and set a non-empty branch_path.
        proof_calculator
            .branch_stack
            .push(ProofTrieBranch { ext_len: 2, state_mask: TrieMask::new(0b1111) });
        proof_calculator
            .branch_stack
            .push(ProofTrieBranch { ext_len: 0, state_mask: TrieMask::new(0b11) });
        proof_calculator.child_stack.push(ProofTrieBranchChild::RlpNode {
            node: RlpNode::word_rlp(&B256::ZERO),
            short_key: Nibbles::new(),
            hash_mask_bit: false,
            tree_mask_bit: false,
        });
        proof_calculator.branch_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);

        // clear_computation_state should reset everything so a subsequent call works.
        proof_calculator.clear_computation_state();

        let mut sorted_slots = slots.to_vec();
        sorted_slots.sort();
        let mut targets: Vec<ProofV2Target> =
            sorted_slots.iter().copied().map(ProofV2Target::new).collect();

        let result = proof_calculator.storage_proof(hashed_address, &mut targets).unwrap();

        // Compare against a fresh calculator to verify correctness.
        let trie_cursor = trie_cursor_factory.storage_trie_cursor(hashed_address).unwrap();
        let hashed_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address).unwrap();
        let mut fresh_calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);
        let fresh_result = fresh_calculator.storage_proof(hashed_address, &mut targets).unwrap();

        pretty_assertions::assert_eq!(fresh_result, result);
    }

    #[test]
    fn test_root_node_reuse_with_overlay() {
        let storage = BTreeMap::from([
            (B256::right_padding_from(&[0x10]), U256::from(1)),
            (B256::right_padding_from(&[0x20]), U256::from(2)),
        ]);
        let harness = ProofTestHarness::new(storage.clone());
        let hashed_address = harness.hashed_address();
        let post_state =
            HashedPostState::from_hashed_storage(hashed_address, HashedStorage::from_iter(storage))
                .into_sorted();
        let trie_updates = TrieUpdatesSorted::default();
        let trie_cursor = InMemoryTrieCursor::new_storage(
            NoopStorageTrieCursor::default(),
            &trie_updates,
            hashed_address,
        );
        let hashed_cursor = HashedPostStateCursor::new_storage(
            NoopHashedCursor::default(),
            &post_state,
            hashed_address,
        );
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        let first_root = calculator.root_node(&mut StorageValueEncoder).unwrap();
        assert!(matches!(first_root.node, TrieNodeV2::Branch(_)));
        assert_eq!(
            calculator.compute_root_hash(core::slice::from_ref(&first_root)).unwrap(),
            Some(harness.original_root())
        );

        let second_root = calculator.root_node(&mut StorageValueEncoder).unwrap();
        pretty_assertions::assert_eq!(first_root, second_root);
    }

    #[test]
    fn test_partial_storage_proof_after_root_calculation() {
        let slot_a = B256::right_padding_from(&[0xae, 0xd4, 0x00]);
        let slot_b = B256::right_padding_from(&[0xae, 0xd4, 0x10]);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (slot_a, U256::from(1)),
            (slot_b, U256::from(2)),
        ]));
        let hashed_address = harness.hashed_address();
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(hashed_address).unwrap();
        let hashed_cursor =
            harness.hashed_cursor_factory().hashed_storage_cursor(hashed_address).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        let root_node = calculator.storage_root_node(hashed_address).unwrap();
        assert_eq!(
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap(),
            Some(harness.original_root())
        );

        let target = ProofV2Target::new(slot_a).with_parent(ProofV2TargetParent::new(3));
        let mut actual_targets = [target];
        let actual = calculator.storage_proof(hashed_address, &mut actual_targets).unwrap();
        let mut expected_targets = [target];
        let (expected, root) = harness.proof_v2(&mut expected_targets);

        assert!(root.is_none());
        pretty_assertions::assert_eq!(expected, actual);
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        /// Generate a strategy for storage datasets (hashed slot → value).
        fn storage_strategy() -> impl Strategy<Value = BTreeMap<B256, U256>> {
            prop::collection::vec((any::<[u8; 32]>(), any::<u64>()), 0..=100).prop_map(|slots| {
                slots
                    .into_iter()
                    .map(|(slot_bytes, value)| (B256::from(slot_bytes), U256::from(value)))
                    .filter(|(_, v)| *v != U256::ZERO)
                    .collect()
            })
        }

        /// Generate a strategy for proof targets that are 80% from existing storage slots
        /// and 20% random keys. Each target has a random parent path length of `None` or 0..15.
        fn proof_targets_strategy(
            slot_keys: Vec<B256>,
        ) -> impl Strategy<Value = Vec<ProofV2Target>> {
            let num_slots = slot_keys.len();

            let target_count = 0..=(num_slots + 5);

            target_count.prop_flat_map(move |count| {
                let slot_keys = slot_keys.clone();
                prop::collection::vec(
                    (
                        prop::bool::weighted(0.8).prop_flat_map(move |from_slots| {
                            if from_slots && !slot_keys.is_empty() {
                                prop::sample::select(slot_keys.clone()).boxed()
                            } else {
                                any::<[u8; 32]>().prop_map(B256::from).boxed()
                            }
                        }),
                        0u8..16u8,
                    )
                        .prop_map(|(key, encoded_parent_path_len)| {
                            let parent = encoded_parent_path_len.checked_sub(1).map_or(
                                ProofV2TargetParent::NONE,
                                |parent_path_len| {
                                    ProofV2TargetParent::new(usize::from(parent_path_len))
                                },
                            );
                            ProofV2Target::new(key).with_parent(parent)
                        }),
                    count,
                )
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(4000))]
            #[test]
            /// Tests that `StorageProofCalculator` produces valid proofs for randomly generated
            /// storage datasets with proof targets.
            fn proptest_proof_with_targets(
                (storage, targets) in storage_strategy()
                    .prop_flat_map(|storage| {
                        let mut slot_keys: Vec<B256> = storage.keys().copied().collect();
                        slot_keys.sort_unstable();
                        let targets_strategy = proof_targets_strategy(slot_keys);
                        (Just(storage), targets_strategy)
                    })
            ) {
                reth_tracing::init_test_tracing();
                let harness = ProofTestHarness::new(storage);

                harness.assert_proof(targets).expect("Proof generation failed");
            }
        }
    }

    #[test]
    fn test_exact_subtrie_targets_with_root_target() {
        reth_tracing::init_test_tracing();

        let slot_80 = B256::right_padding_from(&[0x80]);
        let slot_82 = B256::right_padding_from(&[0x82]);
        let slot_f0 = B256::right_padding_from(&[0xf0]);
        let storage = BTreeMap::from([
            (slot_80, U256::from(1)),
            (slot_82, U256::from(2)),
            (slot_f0, U256::from(3)),
        ]);
        let targets = [
            ProofV2Target::new(B256::ZERO),
            ProofV2Target::new(slot_80).with_parent(ProofV2TargetParent::new(1)),
        ];

        let harness = ProofTestHarness::new(storage);
        harness.assert_proof(targets).expect("Proof generation failed");
    }

    #[test]
    fn test_rebases_singleton_subtrie_root_below_known_parent() {
        let slot = B256::right_padding_from(&[0xae, 0xd4, 0x09]);
        let slot_nibbles = Nibbles::unpack(slot);
        let harness = ProofTestHarness::new(BTreeMap::from([(slot, U256::from(1))]));
        let mut targets = [ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(3))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].path, slot_nibbles.slice(0..4));
        let TrieNodeV2::Leaf(leaf) = &proof[0].node else {
            panic!("singleton subtrie root should remain a leaf")
        };
        assert_eq!(leaf.key, slot_nibbles.slice(4..));
    }

    #[test]
    fn test_rebases_singleton_leaf_at_max_parent_depth() {
        let slot = B256::repeat_byte(0xae);
        let slot_nibbles = Nibbles::unpack(slot);
        let harness = ProofTestHarness::new(BTreeMap::from([(slot, U256::from(1))]));
        let mut targets = [ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(63))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].path, slot_nibbles);
        let TrieNodeV2::Leaf(leaf) = &proof[0].node else {
            panic!("singleton subtrie root should remain a leaf")
        };
        assert!(leaf.key.is_empty());
    }

    #[test]
    fn test_root_and_root_parent_targets_retain_both_singleton_representations() {
        let slot = B256::right_padding_from(&[0x20]);
        let slot_nibbles = Nibbles::unpack(slot);
        let harness = ProofTestHarness::new(BTreeMap::from([(slot, U256::from(1))]));
        let mut targets = [
            ProofV2Target::new(slot),
            ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(0)),
        ];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert_eq!(root, Some(harness.original_root()));
        let root_node = proof.iter().find(|node| node.path.is_empty()).expect("root proof");
        let TrieNodeV2::Leaf(root_leaf) = &root_node.node else { panic!("root should be a leaf") };
        assert_eq!(root_leaf.key, slot_nibbles);

        let child_path = slot_nibbles.slice(0..1);
        let child_node =
            proof.iter().find(|node| node.path == child_path).expect("rebased root child proof");
        let TrieNodeV2::Leaf(child_leaf) = &child_node.node else {
            panic!("root child should be a leaf")
        };
        assert_eq!(child_leaf.key, slot_nibbles.slice(1..));
    }

    #[test]
    fn test_exhausted_cursor_resets_at_equal_target_boundary() {
        let first = B256::ZERO;
        let last = B256::with_last_byte(1);
        let last_nibbles = Nibbles::unpack(last);
        let harness =
            ProofTestHarness::new(BTreeMap::from([(first, U256::from(1)), (last, U256::from(2))]));
        let mut targets = [
            ProofV2Target::new(first),
            ProofV2Target::new(last).with_parent(ProofV2TargetParent::new(63)),
        ];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert_eq!(root, Some(harness.original_root()));
        let child = proof
            .iter()
            .find(|node| node.path == last_nibbles)
            .expect("depth-63 target child proof");
        let TrieNodeV2::Leaf(leaf) = &child.node else { panic!("target child should be a leaf") };
        assert!(leaf.key.is_empty());
    }

    #[test]
    fn test_rebases_compressed_branch_subtrie_root() {
        let slot_a = B256::right_padding_from(&[0xae, 0xd4, 0x00]);
        let slot_b = B256::right_padding_from(&[0xae, 0xd4, 0x10]);
        let slot_nibbles = Nibbles::unpack(slot_a);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (slot_a, U256::from(1)),
            (slot_b, U256::from(2)),
        ]));
        let mut targets = [ProofV2Target::new(slot_a).with_parent(ProofV2TargetParent::new(3))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        let branch_path = slot_nibbles.slice(0..4);
        let branch_node =
            proof.iter().find(|node| node.path == branch_path).expect("rebased compressed branch");
        let TrieNodeV2::Branch(branch) = &branch_node.node else {
            panic!("rebased node should be a branch")
        };
        assert!(branch.key.is_empty());
        assert!(branch.branch_rlp_node.is_none());
    }

    #[test]
    fn test_discards_reconstructed_known_parent_branch() {
        let slot_a = B256::right_padding_from(&[0xae, 0xd2]);
        let slot_b = B256::right_padding_from(&[0xae, 0xd4]);
        let slot_nibbles = Nibbles::unpack(slot_a);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (slot_a, U256::from(1)),
            (slot_b, U256::from(2)),
        ]));
        let mut targets = [ProofV2Target::new(slot_a).with_parent(ProofV2TargetParent::new(3))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert!(!proof.iter().any(|node| node.path == slot_nibbles.slice(0..3)));
        assert!(proof.iter().any(|node| node.path == slot_nibbles.slice(0..4)));
    }

    #[test]
    fn test_rebased_root_matches_direct_child_not_full_short_key() {
        let stored_slot = B256::right_padding_from(&[0xae, 0xd4, 0x09]);
        let same_child_target = B256::right_padding_from(&[0xae, 0xd4, 0xff]);
        let other_child_target = B256::right_padding_from(&[0xae, 0xd5]);
        let harness = ProofTestHarness::new(BTreeMap::from([(stored_slot, U256::from(1))]));

        let mut same_child =
            [ProofV2Target::new(same_child_target).with_parent(ProofV2TargetParent::new(3))];
        let (proof, _) = harness.proof_v2(&mut same_child);
        assert_eq!(proof.len(), 1, "divergent leaf proves absence below the same child");

        let mut other_child =
            [ProofV2Target::new(other_child_target).with_parent(ProofV2TargetParent::new(3))];
        let (proof, _) = harness.proof_v2(&mut other_child);
        assert!(proof.is_empty(), "a different direct child is unrelated to the target");
    }

    #[test]
    fn test_known_parent_sibling_span_retains_only_target_children() {
        let stored_slot_a = B256::right_padding_from(&[0xea, 0x53]);
        let stored_slot_b = B256::right_padding_from(&[0xeb, 0x53]);
        let stored_slot_c = B256::right_padding_from(&[0xec, 0x53]);
        let target_a = B256::right_padding_from(&[0xea, 0x1f]);
        let target_c = B256::right_padding_from(&[0xec, 0x1f]);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (stored_slot_a, U256::from(1)),
            (stored_slot_b, U256::from(2)),
            (stored_slot_c, U256::from(3)),
        ]));
        let mut targets = [target_a, target_c]
            .map(|target| ProofV2Target::new(target).with_parent(ProofV2TargetParent::new(1)));

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(
            proof.iter().map(|node| node.path).collect::<Vec<_>>(),
            [Nibbles::from_nibbles([0xe, 0xa]), Nibbles::from_nibbles([0xe, 0xc])]
        );
    }

    #[test]
    fn test_known_parent_does_not_use_stale_parent_mask() {
        let stored_slot_a = B256::right_padding_from(&[0xea, 0x53]);
        let stored_slot = B256::right_padding_from(&[0xeb, 0x53]);
        let stored_slot_c = B256::right_padding_from(&[0xec, 0x53]);
        let target = B256::right_padding_from(&[0xeb, 0x1f]);
        let stored_slot_nibbles = Nibbles::unpack(stored_slot);

        // The known parent at `e` is supplied by the sparse trie and may be stale in the database
        // when partial persistence masks that path. In particular, its state mask can omit the
        // live `eb` child while hashed state already contains that child's leaf.
        let stale_parent_mask = TrieMask::new((1 << 0xa) | (1 << 0xc));
        let stale_parent = BranchNodeCompact::new(
            stale_parent_mask,
            TrieMask::new(0),
            TrieMask::new(0),
            Vec::new(),
            None,
        );
        let storage_nodes = BTreeMap::from([(Nibbles::from_nibbles([0xe]), stale_parent)]);

        let mut harness = TrieTestHarness::new(BTreeMap::from([
            (stored_slot_a, U256::from(1)),
            (stored_slot, U256::from(2)),
            (stored_slot_c, U256::from(3)),
        ]));
        harness.set_trie_nodes(storage_nodes);

        let mut targets = [ProofV2Target::new(target).with_parent(ProofV2TargetParent::new(1))];
        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].path, stored_slot_nibbles.slice(0..2));
        let TrieNodeV2::Leaf(leaf) = &proof[0].node else {
            panic!("live direct child should be reconstructed as a leaf")
        };
        assert_eq!(leaf.key, stored_slot_nibbles.slice(2..));
    }

    #[test]
    fn test_empty_storage_respects_parent_context() {
        let harness = ProofTestHarness::new(BTreeMap::new());
        let slot = B256::ZERO;

        let mut partial_target =
            [ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(0))];
        let (partial_proof, partial_root) = harness.proof_v2(&mut partial_target);
        assert!(partial_proof.is_empty());
        assert!(partial_root.is_none());

        let mut root_target = [ProofV2Target::new(slot)];
        let (root_proof, root) = harness.proof_v2(&mut root_target);
        assert_eq!(root_proof.len(), 1);
        assert!(matches!(root_proof[0].node, TrieNodeV2::EmptyRoot));
        assert_eq!(root, Some(EMPTY_ROOT_HASH));
    }

    #[test]
    fn test_big_trie() {
        use rand::prelude::*;

        reth_tracing::init_test_tracing();
        let mut rng = rand::rngs::SmallRng::seed_from_u64(1);

        let mut rand_b256 = || {
            let mut buf: [u8; 32] = [0; 32];
            rng.fill_bytes(&mut buf);
            B256::from_slice(&buf)
        };

        // Generate random storage dataset.
        let mut storage = BTreeMap::new();
        for _ in 0..10240 {
            let hashed_slot = rand_b256();
            storage.insert(hashed_slot, U256::from(1u64));
        }

        // Collect targets; partially from real keys, partially random keys which probably won't
        // exist.
        let mut targets = storage.keys().copied().collect::<Vec<_>>();
        for _ in 0..storage.len() / 5 {
            targets.push(rand_b256());
        }
        targets.sort();

        // Create test harness
        let harness = ProofTestHarness::new(storage);

        harness
            .assert_proof(targets.into_iter().map(ProofV2Target::new))
            .expect("Proof generation failed");
    }

    #[test]
    fn test_node_with_masked_empty_child() {
        reth_tracing::init_test_tracing();

        let val = U256::from(42u64);

        // All storage keys share a common first nibble (0x6), so the branch is at path 0x6. The
        // second nibble differentiates children: 0,1,3,5,7.
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_65 = B256::right_padding_from(&[0x65]);
        let slot_67 = B256::right_padding_from(&[0x67]);

        // Construct a branch node at path 0x6 with state_mask bits 0,1,3,5,7.
        // hash_mask has bits 0,1,5,7 (NOT 3) — nibble 3's hash is cleared because it's in the
        // prefix set. Hashes are dummy values.
        let state_mask = TrieMask::new(0b10101011); // bits 0,1,3,5,7
        let hash_mask = TrieMask::new(0b10100011); // bits 0,1,5,7 (NOT 3)
        let hashes = vec![B256::repeat_byte(0xaa); hash_mask.count_ones() as usize];
        let branch = BranchNodeCompact::new(state_mask, TrieMask::new(0), hash_mask, hashes, None);

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x6]), branch)).collect();

        // Hashed cursor has slots at children 0, 1, 5, 7 — but NOT child 3 (0x63).
        // This simulates the post-state overlay having deleted the slot at 0x63.
        let mut harness = TrieTestHarness::new(
            [slot_60, slot_61, slot_65, slot_67].iter().map(|s| (*s, val)).collect(),
        );
        harness.set_trie_nodes(storage_nodes);

        let storage_trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_storage_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor);
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed with masked empty child");

        let root_hash = calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap();
        assert!(root_hash.is_some(), "should produce a root hash");
    }

    /// Tests that `root_node` handles the case where `uncalculated_lower_bound` has advanced
    /// entirely past a cached branch that still has unprocessed children in its `state_mask`.
    ///
    /// Branch at `0x6` has `state_mask` bits 0,1,5,f where nibble 5 has its `hash_mask`
    /// cleared and no leaf data. The last child (nibble f)
    /// causes `calculate_key_range` to be called with range `(0x6f, Some(0x7))`. After that range,
    /// the hashed cursor still has keys (at `0x70...`), so `proof_subtrie` does not break and
    /// re-enters `next_uncached_key_range` with `uncalculated_lower_bound = Some(0x7)`.
    /// Since `0x7` is past `0x6`, all remaining children are skipped and the branch is popped.
    #[test]
    fn test_node_with_masked_empty_child_lower_bound_past_branch() {
        reth_tracing::init_test_tracing();

        let val = U256::from(42u64);

        // Leaf keys under 0x6 and one beyond (0x70) to keep the cursor alive after 0x6.
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_6f = B256::right_padding_from(&[0x6f]);
        let slot_70 = B256::right_padding_from(&[0x70]);

        // Branch at 0x6: state_mask bits 0,1,5,f; hash_mask bits 0,1 (NOT 5, NOT f).
        // Nibble 5 has state_mask set but no hash and no leaf data (masked empty child).
        // Nibble f has state_mask set, no hash, but DOES have leaf data.
        let state_mask = TrieMask::new(0b1000_0000_0010_0011); // bits 0,1,5,f
        let hash_mask = TrieMask::new(0b0000_0000_0000_0011); // bits 0,1
        let hashes = vec![B256::repeat_byte(0xaa); hash_mask.count_ones() as usize];
        let branch = BranchNodeCompact::new(state_mask, TrieMask::new(0), hash_mask, hashes, None);

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x6]), branch)).collect();

        // Hashed cursor: slots at 0x60, 0x61, 0x6f, 0x70 — but NOT 0x65.
        let mut harness = TrieTestHarness::new(
            [slot_60, slot_61, slot_6f, slot_70].iter().map(|s| (*s, val)).collect(),
        );
        harness.set_trie_nodes(storage_nodes);

        let storage_trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_storage_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor);
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed when lower bound advances past branch");

        let root_hash = calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap();
        assert!(root_hash.is_some(), "should produce a root hash");
    }

    /// Tests that the prefix set causes `next_uncached_key_range` to add child nibbles that are
    /// not present in the cached branch's `state_mask`.
    ///
    /// Setup: An original state with leaves at `0x60` and `0x61` produces a cached branch at
    /// `0x6` with children at nibbles 0 and 1 (both with real cached hashes from `StorageRoot`).
    /// A new leaf is then inserted at `0x63...`, which is NOT in the branch's `state_mask`.
    /// The prefix set contains the new key. Without prefix set support, the calculator would
    /// skip nibble 3 entirely and produce a stale root hash. With prefix set support, nibble 3
    /// is discovered and its subtrie is recalculated from leaves.
    #[test]
    fn test_prefix_set_adds_child_nibbles() {
        reth_tracing::init_test_tracing();

        let val = U256::from(42u64);
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_63 = B256::right_padding_from(&[0x63]);

        let harness = TrieTestHarness::new([(slot_60, val), (slot_61, val)].into_iter().collect());

        let changeset: BTreeMap<B256, U256> = std::iter::once((slot_63, val)).collect();
        let (expected_root, _) = harness.get_root_with_updates(&changeset);

        let mut updated_storage = harness.storage().clone();
        updated_storage.insert(slot_63, val);

        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), updated_storage)).collect(),
        );

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(slot_63));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed with prefix set adding child nibbles");
        let got_root =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash with prefix set should match fresh computation"
        );
    }

    /// Tests that `next_uncached_key_range` does not use a cached hash when the child's path
    /// is in the prefix set, forcing recalculation from leaves.
    ///
    /// Setup: A cached branch at `0x6` with children at nibbles 0,1,5 — all with cached hashes.
    /// The leaf at `0x65...` is changed (different value). The prefix set marks `0x65...` as
    /// dirty. Without prefix set support, the calculator would use the stale cached hash for
    /// nibble 5 and produce a wrong root. With prefix set support, the cached hash is skipped
    /// and the subtrie is recalculated from the updated leaf data.
    #[test]
    fn test_prefix_set_invalidates_cached_hash() {
        reth_tracing::init_test_tracing();

        let original_val = U256::from(42u64);
        let updated_val = U256::from(9999u64);
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_65 = B256::right_padding_from(&[0x65]);

        let harness = TrieTestHarness::new(
            [(slot_60, original_val), (slot_61, original_val), (slot_65, original_val)]
                .into_iter()
                .collect(),
        );

        let changeset: BTreeMap<B256, U256> = std::iter::once((slot_65, updated_val)).collect();
        let (expected_root, _) = harness.get_root_with_updates(&changeset);

        let mut updated_storage = harness.storage().clone();
        updated_storage.insert(slot_65, updated_val);

        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), updated_storage)).collect(),
        );

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(slot_65));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed with prefix set invalidating cached hash");
        let got_root =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash with prefix set invalidation should match fresh computation"
        );
    }

    fn b256(s: &str) -> B256 {
        B256::from_slice(&alloy_primitives::hex::decode(s).expect("valid hex string"))
    }

    #[test]
    fn test_prefix_set_root_proof_processes_sibling_after_cached_descendant() {
        reth_tracing::init_test_tracing();

        let storage = [
            ("1022c69e9d900e40775cd387c134899f465f291dbc3c97899ff6bfb8dc972b37", 45u64),
            ("1111ad8083c8a3a398b2b781217b989ff4d1ed182f46cc765eda49a7b316139d", 60),
            ("12012d20943649899b2fc0f87b9840b70ef68e93613aac17c269bf8c5a78a712", 17),
            ("12014b57b9a162c03d072eb6acd4e936f1c4bc23b803a054347c5ee9a9bcfb9a", 49),
            ("1203f800840af3f898ab4572f2750106a7c4bd2b3e844b6e7fa72704673cc2c6", 76),
            ("12208f18fbcd6971c92808721392acbf11d5af58e9143a374cc86e70bdd1f097", 10),
        ]
        .into_iter()
        .map(|(key, value)| (b256(key), U256::from(value)))
        .collect();

        let dirty = b256("12208f18fbcd6971c92808721392acbf11d5af58e9143a374cc86e70bdd1f097");
        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(dirty));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
            "root proof must process a prefix-set sibling after a cached descendant",
        );
    }

    #[test]
    fn test_prefix_set_root_proof_processes_trailing_dirty_sibling() {
        reth_tracing::init_test_tracing();

        let keys = [
            "0022001020000000000000000000000000000000000000000000000000000000",
            "0110212112000000000000000000000000000000000000000000000000000000",
            "0202210210000000000000000000000000000000000000000000000000000000",
            "0211020211000000000000000000000000000000000000000000000000000000",
            "0211211002000000000000000000000000000000000000000000000000000000",
            "0212221010000000000000000000000000000000000000000000000000000000",
            "0222011102000000000000000000000000000000000000000000000000000000",
        ];
        let storage =
            keys.iter().enumerate().map(|(i, key)| (b256(key), U256::from(i as u64 + 1))).collect();
        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();

        // The dirty children straddle a clean cached descendant under branch 0x02. Traversal
        // must resume at the trailing dirty sibling after using the cached descendant.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(b256(keys[2])));
        prefix_set.insert(Nibbles::unpack(b256(keys[6])));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
        );
    }

    /// Helper to compute the keccak256 hash of a storage leaf node. The `short_key` is the
    /// leaf's key after trimming all branch/extension nibbles consumed by ancestor nodes.
    fn storage_leaf_hash(short_key: &Nibbles, value: &U256) -> B256 {
        let mut buf = Vec::new();
        alloy_trie::nodes::LeafNodeRef::new(short_key, &alloy_rlp::encode_fixed_size(value))
            .encode(&mut buf);
        keccak256(&buf)
    }

    /// Checks collapsing a cached branch after removing one of its two direct branch children.
    fn assert_branch_collapse(remaining_nibble: u8, removed_nibble: u8) {
        reth_tracing::init_test_tracing();

        let val = U256::from(1u64);
        let child_keys = |nibble| {
            [
                B256::right_padding_from(&[0x20 | nibble, 0x00]),
                B256::right_padding_from(&[0x20 | nibble, 0x10]),
            ]
        };
        let [remaining_a, remaining_b] = child_keys(remaining_nibble);
        let [removed_a, removed_b] = child_keys(removed_nibble);

        // Build the cached trie through the production HashBuilder path. Each child at
        // `remaining_nibble` and `removed_nibble` is a direct branch with two leaf children.
        let initial_storage = BTreeMap::from([
            (remaining_a, val),
            (remaining_b, val),
            (removed_a, val),
            (removed_b, val),
        ]);
        let harness = TrieTestHarness::new(initial_storage);

        let cached_branch = &harness
            .storage_trie_updates()
            .storage_nodes
            .get(&Nibbles::from_nibbles([0x2]))
            .expect("branch at 0x2");
        let child_mask =
            TrieMask::from_nibble(remaining_nibble) | TrieMask::from_nibble(removed_nibble);
        assert_eq!(cached_branch.state_mask, child_mask);
        assert_eq!(cached_branch.hash_mask, child_mask);
        assert!(cached_branch.tree_mask.is_empty());

        let final_storage = BTreeMap::from([(remaining_a, val), (remaining_b, val)]);
        let expected_root = TrieTestHarness::new(final_storage.clone()).original_root();
        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), final_storage)).collect(),
        );

        // The prefix set marks the entire removed child subtree dirty.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(removed_a));
        prefix_set.insert(Nibbles::unpack(removed_b));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed after branch collapse");
        let root =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        pretty_assertions::assert_eq!(expected_root, root);
    }

    #[test]
    fn test_branch_collapse_removed_child_before_remaining() {
        assert_branch_collapse(1, 0);
    }

    #[test]
    fn test_branch_collapse_removed_child_after_remaining() {
        assert_branch_collapse(4, 9);
    }

    #[test]
    fn test_prefix_set_root_proof_preserves_clean_sibling_after_cached_branch_collapse() {
        reth_tracing::init_test_tracing();

        let dirty = B256::right_padding_from(&[0x10, 0x00, 0x10]);
        let clean_sibling = B256::right_padding_from(&[0x10, 0x10]);
        let storage = [
            B256::right_padding_from(&[0x10]),
            dirty,
            B256::right_padding_from(&[0x10, 0x01]),
            B256::right_padding_from(&[0x10, 0x02]),
            clean_sibling,
            B256::right_padding_from(&[0x11]),
            B256::right_padding_from(&[0x12]),
        ]
        .into_iter()
        .map(|key| (key, U256::from(1u64)))
        .collect();

        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(dirty));

        let mut prefix_set_with_sibling = PrefixSetMut::default();
        prefix_set_with_sibling.insert(Nibbles::unpack(dirty));
        prefix_set_with_sibling.insert(Nibbles::unpack(clean_sibling));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set_with_sibling.freeze()),
        );
        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
            "a dirty prefix must not omit a clean sibling after collapsing a cached branch",
        );
    }

    #[test]
    fn test_prefix_set_range_skips_covered_cached_branch() {
        reth_tracing::init_test_tracing();

        let before = B256::right_padding_from(&[0x30]);
        let cached_a = B256::right_padding_from(&[0x80, 0x10]);
        let cached_b = B256::right_padding_from(&[0x80, 0x15]);
        let dirty = B256::right_padding_from(&[0x80, 0xc0]);
        let after = B256::right_padding_from(&[0x90]);

        // The cached branch at 0x80 remains parked while its children are rebuilt. Processing the
        // dirty child advances the lower bound to 0x9, past the cached branch's entire range.
        let storage = [before, cached_a, cached_b, dirty, after]
            .into_iter()
            .enumerate()
            .map(|(i, key)| (key, U256::from(i + 1)))
            .collect();

        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(dirty));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
        );
    }

    #[test]
    fn test_cached_branch_extension_skips_diverging_target() {
        reth_tracing::init_test_tracing();

        let val = U256::from(100u64);

        // Keys whose first bytes directly set the nibble paths we need.
        let key_a0 = B256::right_padding_from(&[0x6a, 0x30]); // nibbles: 6,a,3,0,...
        let key_a1 = B256::right_padding_from(&[0x6a, 0x31]); // nibbles: 6,a,3,1,...
        let key_c = B256::right_padding_from(&[0x6a, 0x80]); // nibbles: 6,a,8,0,...
        let key_d = B256::right_padding_from(&[0x6b, 0x00]); // nibbles: 6,b,0,0,...
        let key_e = B256::right_padding_from(&[0x6c, 0x00]); // nibbles: 6,c,0,0,...

        // Build a correct trie from all five leaves to get the expected root and real hashes.
        let all_storage: BTreeMap<B256, U256> =
            [(key_a0, val), (key_a1, val), (key_c, val), (key_d, val), (key_e, val)]
                .into_iter()
                .collect();
        let correct_harness = TrieTestHarness::new(all_storage.clone());
        let expected_root = correct_harness.original_root();

        // Compute leaf hashes for constructing manual cached branch nodes.
        let leaf_hash_a0 = storage_leaf_hash(&Nibbles::unpack(key_a0).slice(4..), &val);
        let leaf_hash_a1 = storage_leaf_hash(&Nibbles::unpack(key_a1).slice(4..), &val);
        let leaf_hash_d = storage_leaf_hash(&Nibbles::unpack(key_d).slice(2..), &val);
        let leaf_hash_e = storage_leaf_hash(&Nibbles::unpack(key_e).slice(2..), &val);

        // ── Construct cached branch at [6] ─────────────────────────────────────
        // state_mask: bits a, b, and c set.
        // hash_mask:  bits b and c — both have cached leaf hashes.  Bit a has no hash, so the
        //             calculator will seek the trie cursor to find a deeper cached branch.
        //
        // Children b and c remain clean and can still use their cached hashes.
        let branch_6_state_mask = TrieMask::new((1 << 0xa) | (1 << 0xb) | (1 << 0xc));
        let branch_6_hash_mask = TrieMask::new((1 << 0xb) | (1 << 0xc));
        let branch_6 = BranchNodeCompact::new(
            branch_6_state_mask,
            TrieMask::new(0),
            branch_6_hash_mask,
            vec![leaf_hash_d, leaf_hash_e],
            None,
        );

        // ── Construct cached branch at [6,a,3] ────────────────────────────────
        // state_mask: bits 0 and 1 set (children key_a0 and key_a1).
        // hash_mask:  both bits set — both children have cached hashes.
        let branch_6a3_state_mask = TrieMask::new((1 << 0) | (1 << 1));
        let branch_6a3 = BranchNodeCompact::new(
            branch_6a3_state_mask,
            TrieMask::new(0),
            branch_6a3_state_mask,
            vec![leaf_hash_a0, leaf_hash_a1],
            None,
        );

        // Intentionally omit the branch at [6,a] — this is the inconsistency.
        let inconsistent_nodes: BTreeMap<Nibbles, BranchNodeCompact> = [
            (Nibbles::from_nibbles([0x6]), branch_6),
            (Nibbles::from_nibbles([0x6, 0xa, 0x3]), branch_6a3),
        ]
        .into_iter()
        .collect();

        // Create harness with all five leaves but the inconsistent trie nodes.
        let mut harness = TrieTestHarness::new(all_storage);
        harness.set_trie_nodes(inconsistent_nodes);

        // Mark key_c as dirty — in the real scenario the leaf was touched by execution. The
        // nested branch [6,a,3] remains clean because key_c diverges at [6,a,8].
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_c));

        // ── Verify root hash ───────────────────────────────────────────────────
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());

        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed");
        let got_root = calculator
            .compute_root_hash(core::slice::from_ref(&root_node))
            .unwrap()
            .expect("should produce a root hash");

        // With the bug, the calculator skips key_c and produces a wrong root.
        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash should match correct trie; cached extension must not skip diverging leaves"
        );

        // ── Verify proof for key_c contains nodes on its path ──────────────────
        let mut targets = vec![ProofV2Target::new(key_c)];
        let proofs = calculator
            .storage_proof(harness.hashed_address(), &mut targets)
            .expect("storage_proof should succeed");

        let key_c_nibbles = Nibbles::unpack(key_c);
        let has_matching_node = proofs.iter().any(|node| key_c_nibbles.starts_with(&node.path));
        assert!(
            has_matching_node,
            "Proof for key_c should contain at least one node on key_c's path, got: {proofs:?}"
        );
    }

    #[test]
    fn test_cached_branch_extension_skips_diverging_target_before() {
        reth_tracing::init_test_tracing();

        let val = U256::from(100u64);

        // Keys whose first bytes directly set the nibble paths we need.
        let key_a0 = B256::right_padding_from(&[0x6a, 0x80]); // nibbles: 6,a,8,0,...
        let key_a1 = B256::right_padding_from(&[0x6a, 0x81]); // nibbles: 6,a,8,1,...
        let key_c = B256::right_padding_from(&[0x6a, 0x30]); // nibbles: 6,a,3,0,... (BEFORE
                                                             // [6,a,8])
        let key_d = B256::right_padding_from(&[0x6b, 0x00]); // nibbles: 6,b,0,0,...
        let key_e = B256::right_padding_from(&[0x6c, 0x00]); // nibbles: 6,c,0,0,...

        // Build a correct trie from all five leaves to get the expected root and real hashes.
        let all_storage: BTreeMap<B256, U256> =
            [(key_a0, val), (key_a1, val), (key_c, val), (key_d, val), (key_e, val)]
                .into_iter()
                .collect();
        let correct_harness = TrieTestHarness::new(all_storage.clone());
        let expected_root = correct_harness.original_root();

        // Compute leaf hashes for constructing manual cached branch nodes.
        let leaf_hash_a0 = storage_leaf_hash(&Nibbles::unpack(key_a0).slice(4..), &val);
        let leaf_hash_a1 = storage_leaf_hash(&Nibbles::unpack(key_a1).slice(4..), &val);
        let leaf_hash_d = storage_leaf_hash(&Nibbles::unpack(key_d).slice(2..), &val);
        let leaf_hash_e = storage_leaf_hash(&Nibbles::unpack(key_e).slice(2..), &val);

        // ── Construct cached branch at [6] ─────────────────────────────────────
        // state_mask: bits a, b, and c set.
        // hash_mask:  bits b and c — both have cached leaf hashes.  Bit a has no hash, so the
        //             calculator will seek the trie cursor to find a deeper cached branch.
        //
        // Children b and c remain clean and can still use their cached hashes.
        let branch_6_state_mask = TrieMask::new((1 << 0xa) | (1 << 0xb) | (1 << 0xc));
        let branch_6_hash_mask = TrieMask::new((1 << 0xb) | (1 << 0xc));
        let branch_6 = BranchNodeCompact::new(
            branch_6_state_mask,
            TrieMask::new(0),
            branch_6_hash_mask,
            vec![leaf_hash_d, leaf_hash_e],
            None,
        );

        // ── Construct cached branch at [6,a,8] ────────────────────────────────
        // state_mask: bits 0 and 1 set (children key_a0 and key_a1).
        // hash_mask:  both bits set — both children have cached hashes.
        let branch_6a8_state_mask = TrieMask::new((1 << 0) | (1 << 1));
        let branch_6a8 = BranchNodeCompact::new(
            branch_6a8_state_mask,
            TrieMask::new(0),
            branch_6a8_state_mask,
            vec![leaf_hash_a0, leaf_hash_a1],
            None,
        );

        // Intentionally omit the branch at [6,a] — this is the inconsistency.
        let inconsistent_nodes: BTreeMap<Nibbles, BranchNodeCompact> = [
            (Nibbles::from_nibbles([0x6]), branch_6),
            (Nibbles::from_nibbles([0x6, 0xa, 0x8]), branch_6a8),
        ]
        .into_iter()
        .collect();

        // Create harness with all five leaves but the inconsistent trie nodes.
        let mut harness = TrieTestHarness::new(all_storage);
        harness.set_trie_nodes(inconsistent_nodes);

        // Mark key_c as dirty — it comes BEFORE the cached branch [6,a,8] in nibble order.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_c));

        // ── Verify root hash ───────────────────────────────────────────────────
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());

        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed");
        let got_root = calculator
            .compute_root_hash(core::slice::from_ref(&root_node))
            .unwrap()
            .expect("should produce a root hash");

        // With the bug, the calculator skips key_c and produces a wrong root.
        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash should match correct trie; cached extension must not skip diverging leaves before cached branch"
        );

        // ── Verify proof for key_c contains nodes on its path ──────────────────
        let mut targets = vec![ProofV2Target::new(key_c)];
        let proofs = calculator
            .storage_proof(harness.hashed_address(), &mut targets)
            .expect("storage_proof should succeed");

        let key_c_nibbles = Nibbles::unpack(key_c);
        let has_matching_node = proofs.iter().any(|node| key_c_nibbles.starts_with(&node.path));
        assert!(
            has_matching_node,
            "Proof for key_c should contain at least one node on key_c's path, got: {proofs:?}"
        );
    }

    #[test]
    fn test_skipped_parent_branch_with_unskipped_child() {
        reth_tracing::init_test_tracing();

        let val = U256::from(1u64);
        let updated_val = U256::from(2u64);

        // We need cached branches at [2], [2,f], and [3] in the trie DB.
        let key_2 = B256::right_padding_from(&[0x20]);
        let key_2f00 = B256::right_padding_from(&[0x2f, 0x00]);
        let key_2f01 = B256::right_padding_from(&[0x2f, 0x01]);
        let key_2f10 = B256::right_padding_from(&[0x2f, 0x10]);
        let key_2f11 = B256::right_padding_from(&[0x2f, 0x11]);
        let key_300 = B256::right_padding_from(&[0x30, 0x00]);
        let key_301 = B256::right_padding_from(&[0x30, 0x10]);
        let key_310 = B256::right_padding_from(&[0x31, 0x00]);
        let key_311 = B256::right_padding_from(&[0x31, 0x10]);
        let key_500 = B256::right_padding_from(&[0x50, 0x00]);
        let key_501 = B256::right_padding_from(&[0x50, 0x10]);
        let key_510 = B256::right_padding_from(&[0x51, 0x00]);
        let key_511 = B256::right_padding_from(&[0x51, 0x10]);

        let all_keys = [
            key_2, key_2f00, key_2f01, key_2f10, key_2f11, key_300, key_301, key_310, key_311,
            key_500, key_501, key_510, key_511,
        ];

        let original_storage: BTreeMap<B256, U256> = all_keys.iter().map(|k| (*k, val)).collect();
        let harness = TrieTestHarness::new(original_storage);

        // Verify that the expected branches exist in the trie.
        let trie_updates = harness.storage_trie_updates();
        assert!(trie_updates.storage_nodes.contains_key(&Nibbles::from_nibbles([0x2])));
        assert!(trie_updates.storage_nodes.contains_key(&Nibbles::from_nibbles([0x2, 0xf])));
        assert!(trie_updates.storage_nodes.contains_key(&Nibbles::from_nibbles([0x3])));

        // Change only key_2 — triggers skip of parent branch [2] while child [2,f] is not
        // skipped.
        let changeset: BTreeMap<B256, U256> = std::iter::once((key_2, updated_val)).collect();
        let (expected_root, _) = harness.get_root_with_updates(&changeset);

        let mut updated_storage = harness.storage().clone();
        updated_storage.insert(key_2, updated_val);

        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), updated_storage)).collect(),
        );

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_2));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed");

        let got_root = calculator
            .compute_root_hash(&[root_node])
            .expect("root hash should succeed")
            .expect("root should get hashed");
        pretty_assertions::assert_eq!(expected_root, got_root);
    }

    #[test]
    fn test_blinded_local_root_returns_trie_inconsistency() {
        let key = B256::right_padding_from(&[0x63, 0xaa]);
        let value = U256::from(1);
        let hash = storage_leaf_hash(&Nibbles::unpack(key).slice(2..), &value);
        let mask = TrieMask::from_nibble(3);
        let cached_branch =
            BranchNodeCompact::new(mask, TrieMask::default(), mask, vec![hash], None);

        let mut harness = TrieTestHarness::new(BTreeMap::from([(key, value)]));
        harness.set_trie_nodes(BTreeMap::from([(Nibbles::from_nibbles([6]), cached_branch)]));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        assert!(matches!(
            calculator.storage_root_node(harness.hashed_address()),
            Err(StateProofError::TrieInconsistency(_))
        ));
    }

    #[test]
    fn test_cached_hash_with_deleted_leaf() {
        reth_tracing::init_test_tracing();

        // Use different values to ensure distinct leaf hashes.
        let val_3 = U256::from(111u64);
        let val_5 = U256::from(222u64);
        let val_8 = U256::from(333u64);

        // Keys under a common prefix `0x6_` to create a branch at path [6].
        // Use second byte to distinguish short keys (so they differ after position 2).
        let key_63 = B256::right_padding_from(&[0x63, 0xaa]); // nibble path: 6,3,a,a,...
        let key_65 = B256::right_padding_from(&[0x65, 0xbb]); // nibble path: 6,5,b,b,...
        let key_68 = B256::right_padding_from(&[0x68, 0xcc]); // nibble path: 6,8,c,c,...

        // Compute leaf hashes. The branch at [6] consumes 2 nibbles (the branch path [6]
        // plus the child nibble), so each leaf's short key starts at position 2.
        let leaf_hash_3 = storage_leaf_hash(&Nibbles::unpack(key_63).slice(2..), &val_3);
        let leaf_hash_5 = storage_leaf_hash(&Nibbles::unpack(key_65).slice(2..), &val_5);
        let leaf_hash_8 = storage_leaf_hash(&Nibbles::unpack(key_68).slice(2..), &val_8);

        // Build cached branch at [6] with state_mask and hash_mask bits for nibbles 3, 5, 8.
        let state_mask = TrieMask::new((1 << 3) | (1 << 5) | (1 << 8));
        let cached_branch = BranchNodeCompact::new(
            state_mask,
            TrieMask::new(0),
            state_mask, // hash_mask = state_mask (all children have cached hashes)
            vec![leaf_hash_3, leaf_hash_5, leaf_hash_8],
            None,
        );

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x6]), cached_branch)).collect();

        // Compute the expected root from a fresh trie with just key_65 and key_68.
        let mut harness =
            TrieTestHarness::new([(key_65, val_5), (key_68, val_8)].into_iter().collect());
        let expected_root = harness.original_root();

        // Update the harness with a cached trie node which will reference key_63 by hash.
        harness.set_trie_nodes(storage_nodes);

        // Mark key_63 as dirty in the prefix set — in the real scenario the leaf was
        // deleted and the HashedPostState overlay masks it out.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_63));

        // Request a proof for key_63 (absence proof — no leaf exists).
        // Because the prefix set marks nibble 3's child path as dirty, the cached hash for
        // nibble 3 is skipped.
        let mut targets = vec![ProofV2Target::new(key_63)];

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());

        let proofs = calculator
            .storage_proof(harness.hashed_address(), &mut targets)
            .expect("storage_proof should succeed");
        assert_eq!(1, proofs.len());
        let got_root = calculator
            .compute_root_hash(&proofs)
            .expect("compute_root_hash should succeed")
            .expect("should produce a root hash (proof contains root node)");

        // With the bug, nibble 5 gets hashes[0] (nibble 3's hash) and nibble 8 gets
        // hashes[1] (nibble 5's hash), producing a wrong root.
        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash should match trie without key_63; cached hash index is off when \
             an earlier hashed child has no leaves (absence proof target)"
        );
    }
}
