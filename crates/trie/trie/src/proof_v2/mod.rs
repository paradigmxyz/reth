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
use reth_trie_common::{BranchNode, BranchNodeMasks, Nibbles, ProofTrieNode, RlpNode, TrieNode};
use std::cmp::Ordering;
use tracing::{error, instrument, trace};

mod value;
pub use value::*;

mod node;
use node::*;

mod target;
pub use target::*;

/// Target to use with the `tracing` crate.
static TRACE_TARGET: &str = "trie::proof_v2";

/// Number of bytes to pre-allocate for [`ProofCalculator`]'s `rlp_encode_buf` field.
const RLP_ENCODE_BUF_SIZE: usize = 1024;

/// A [`Nibbles`] which contains 64 zero nibbles.
static PATH_ALL_ZEROS: Nibbles = {
    let mut path = Nibbles::new();
    let mut i = 0;
    while i < 64 {
        path.push_unchecked(0);
        i += 1;
    }
    path
};

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
    /// Children on the `child_stack` are converted to [`ProofTrieBranchChild::RlpNode`]s via the
    /// [`Self::commit_child`] method. Committing a child indicates that no further changes are
    /// expected to happen to it (e.g. splitting its short key when inserting a new branch). Given
    /// that keys are consumed in lexicographical order, only the last child on the stack can
    /// ever be modified, and therefore all children besides the last are expected to be
    /// [`ProofTrieBranchChild::RlpNode`]s.
    child_stack: Vec<ProofTrieBranchChild<VE::DeferredEncoder>>,
    /// Cached branch data pulled from the `trie_cursor`. The calculator will use the cached
    /// [`BranchNodeCompact::hashes`] to skip over the calculation of sub-tries in the overall
    /// trie. The cached hashes cannot be used for any paths which are prefixes of a proof target.
    cached_branch_stack: Vec<(Nibbles, BranchNodeCompact)>,
    /// The proofs which will be returned from the calculation. This gets taken at the end of every
    /// proof call.
    retained_proofs: Vec<ProofTrieNode>,
    /// Free-list of re-usable buffers of [`RlpNode`]s, used for encoding branch nodes to RLP.
    ///
    /// We are generally able to re-use these buffers across different branch nodes for the
    /// duration of a proof calculation, but occasionally we will lose one when a branch
    /// node is returned as a `ProofTrieNode`.
    rlp_nodes_bufs: Vec<Vec<RlpNode>>,
    /// Re-usable byte buffer, used for RLP encoding.
    rlp_encode_buf: Vec<u8>,
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
    fn should_retain<'a>(
        &self,
        targets: &mut Option<TargetsCursor<'a>>,
        path: &Nibbles,
        check_min_len: bool,
    ) -> bool {
        // If no targets are given then we never retain anything
        let Some(targets) = targets.as_mut() else { return false };

        let (mut lower, mut upper) = targets.current();

        trace!(target: TRACE_TARGET, ?path, target = ?lower, "should_retain: called");
        debug_assert!(self.retained_proofs.last().is_none_or(
                |ProofTrieNode { path: last_retained_path, .. }| {
                    depth_first::cmp(path, last_retained_path) == Ordering::Greater
                }
            ),
            "should_retain called with path {path:?} which is not after previously retained node {:?} in depth-first order",
            self.retained_proofs.last().map(|n| n.path),
        );

        loop {
            // If the node in question is a prefix of the target then we do not iterate targets
            // further.
            //
            // Even if the node is a prefix of the target's key, if the target has a non-zero
            // `min_len` it indicates that the node should only be retained if it is
            // longer than that value.
            //
            // _However_ even if the node doesn't match the target due to the target's `min_len`, it
            // may match other targets whose keys match this node. So we search forwards and
            // backwards for all targets which might match this node, and check against the
            // `min_len` of each.
            //
            // For example, given a branch 0xabc, with children at 0, 1, and 2, and targets:
            // - key: 0xabc0, min_len: 2
            // - key: 0xabc1, min_len: 1
            // - key: 0xabc2, min_len: 4 <-- current
            // - key: 0xabc3, min_len: 3
            //
            // When the branch node at 0xabc is visited it will be after the targets has iterated
            // forward to 0xabc2 (because all children will have been visited already). At this
            // point the target for 0xabc2 will not match the branch due to its prefix, but any of
            // the other targets would, so we need to check those as well.
            if lower.key.starts_with(path) {
                return !check_min_len ||
                    (path.len() >= lower.min_len as usize ||
                        targets
                            .skip_iter()
                            .take_while(|target| target.key.starts_with(path))
                            .any(|target| path.len() >= target.min_len as usize) ||
                        targets
                            .rev_iter()
                            .take_while(|target| target.key.starts_with(path))
                            .any(|target| path.len() >= target.min_len as usize))
            }

            // If the path isn't in the current range then iterate forward until it is (or until
            // there is no upper bound, indicating unbounded).
            if upper.is_some_and(|upper| depth_first::cmp(path, &upper.key) != Ordering::Less) {
                (lower, upper) = targets.next();
                trace!(target: TRACE_TARGET, target = ?lower, "upper target <= path, next target");
            } else {
                return false
            }
        }
    }

    /// Takes a child which has been removed from the `child_stack` and converts it to an
    /// [`RlpNode`].
    ///
    /// Calling this method indicates that the child will not undergo any further modifications, and
    /// therefore can be retained as a proof node if applicable.
    fn commit_child<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        child_path: Nibbles,
        child: ProofTrieBranchChild<VE::DeferredEncoder>,
    ) -> Result<RlpNode, StateProofError> {
        // If the child is already an `RlpNode` then there is nothing to do.
        if let ProofTrieBranchChild::RlpNode(rlp_node) = child {
            return Ok(rlp_node)
        }

        // If we should retain the child then do so.
        if self.should_retain(targets, &child_path, true) {
            trace!(target: TRACE_TARGET, ?child_path, "Retaining child");

            // Convert to `ProofTrieNode`, which will be what is retained.
            //
            // If this node is a branch then its `rlp_nodes_buf` will be taken and not returned to
            // the `rlp_nodes_bufs` free-list.
            self.rlp_encode_buf.clear();
            let proof_node = child.into_proof_trie_node(child_path, &mut self.rlp_encode_buf)?;

            // Use the `ProofTrieNode` to encode the `RlpNode`, and then push it onto retained
            // nodes before returning.
            self.rlp_encode_buf.clear();
            proof_node.node.encode(&mut self.rlp_encode_buf);

            self.retained_proofs.push(proof_node);
            return Ok(RlpNode::from_rlp(&self.rlp_encode_buf));
        }

        // If the child path is not being retained then we convert directly to an `RlpNode`
        // using `into_rlp`. Since we are not retaining the node we can recover any `RlpNode`
        // buffers for the free-list here, hence why we do this as a separate logical branch.
        self.rlp_encode_buf.clear();
        let (child_rlp_node, freed_rlp_nodes_buf) = child.into_rlp(&mut self.rlp_encode_buf)?;

        // If there is an `RlpNode` buffer which can be re-used then push it onto the free-list.
        if let Some(buf) = freed_rlp_nodes_buf {
            self.rlp_nodes_bufs.push(buf);
        }

        Ok(child_rlp_node)
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

    /// Calls [`Self::commit_child`] on the last child of `child_stack`, replacing it with a
    /// [`ProofTrieBranchChild::RlpNode`].
    ///
    /// If `child_stack` is empty then this is a no-op.
    ///
    /// NOTE that this method call relies on the `state_mask` of the top branch of the
    /// `branch_stack` to determine the last child's path. When committing the last child prior to
    /// pushing a new child, it's important to set the new child's `state_mask` bit _after_ the call
    /// to this method.
    fn commit_last_child<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
    ) -> Result<(), StateProofError> {
        let Some(child_path) = self.last_child_path() else { return Ok(()) };
        let child =
            self.child_stack.pop().expect("child_stack can't be empty if there's a child path");

        // If the child is already an `RlpNode` then there is nothing to do, push it back on with no
        // changes.
        if let ProofTrieBranchChild::RlpNode(_) = child {
            self.child_stack.push(child);
            return Ok(())
        }

        // Only commit immediately if retained for the proof. Otherwise, defer conversion
        // to pop_branch() to give DeferredEncoder time for async work.
        if self.should_retain(targets, &child_path, true) {
            let child_rlp_node = self.commit_child(targets, child_path, child)?;
            self.child_stack.push(ProofTrieBranchChild::RlpNode(child_rlp_node));
        } else {
            self.child_stack.push(child);
        }

        Ok(())
    }

    /// Creates a new leaf node on a branch, setting its `state_mask` bit and pushing the leaf onto
    /// the `child_stack`.
    ///
    /// # Panics
    ///
    /// - If `branch_stack` is empty
    /// - If the leaf's nibble is already set in the branch's `state_mask`.
    fn push_new_leaf<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        leaf_nibble: u8,
        leaf_short_key: Nibbles,
        leaf_val: VE::DeferredEncoder,
    ) -> Result<(), StateProofError> {
        // Before pushing the new leaf onto the `child_stack` we need to commit the previous last
        // child, so that only `child_stack`'s final child is a non-RlpNode.
        self.commit_last_child(targets)?;

        // Once the last child is committed we set the new child's bit on the top branch's
        // `state_mask` and push that new child.
        let branch = self.branch_stack.last_mut().expect("branch_stack cannot be empty");

        debug_assert!(!branch.state_mask.is_bit_set(leaf_nibble));
        branch.state_mask.set_bit(leaf_nibble);

        self.child_stack
            .push(ProofTrieBranchChild::Leaf { short_key: leaf_short_key, value: leaf_val });

        Ok(())
    }

    /// Pushes a new branch onto the `branch_stack` based on the path and short key of the last
    /// child on the `child_stack` and the path of the next child which will be pushed on to the
    /// stack after this call.
    ///
    /// Returns the nibble of the branch's `state_mask` which should be set for the new child, and
    /// short key that the next child should use.
    fn push_new_branch(&mut self, new_child_path: Nibbles) -> (u8, Nibbles) {
        // First determine the new child's shortkey relative to the current branch. If there is no
        // current branch then the short key is the full path.
        let new_child_short_key = if self.branch_stack.is_empty() {
            new_child_path
        } else {
            // When there is a current branch then trim off its path as well as the nibble that it
            // has set for this leaf.
            trim_nibbles_prefix(&new_child_path, self.branch_path.len() + 1)
        };

        // Get the new branch's first child, which is the child on the top of the stack with which
        // the new child shares the same nibble on the current branch.
        let first_child = self
            .child_stack
            .last_mut()
            .expect("push_new_branch can't be called with empty child_stack");

        let first_child_short_key = first_child.short_key();
        debug_assert!(
            !first_child_short_key.is_empty(),
            "push_new_branch called when top child on stack is not a leaf or extension with a short key",
        );

        // Determine how many nibbles are shared between the new branch's first child and the new
        // child. This common prefix will be the extension of the new branch
        let common_prefix_len = first_child_short_key.common_prefix_length(&new_child_short_key);

        // Trim off the common prefix from the first child's short key, plus one nibble which will
        // stored by the new branch itself in its state mask.
        let first_child_nibble = first_child_short_key.get_unchecked(common_prefix_len);
        first_child.trim_short_key_prefix(common_prefix_len + 1);

        // Similarly, trim off the common prefix, plus one nibble for the new branch, from the new
        // child's short key.
        let new_child_nibble = new_child_short_key.get_unchecked(common_prefix_len);
        let new_child_short_key = trim_nibbles_prefix(&new_child_short_key, common_prefix_len + 1);

        // Update the branch path to reflect the new branch about to be pushed. Its path will be
        // the path of the previous branch, plus the nibble shared by each child, plus the parent
        // extension (denoted by a non-zero `ext_len`). Since the new branch's path is a prefix of
        // the original new_child_path we can just slice that.
        //
        // If the new branch is the first branch then we do not add the extra 1, as there is no
        // nibble in a parent branch to account for.
        let branch_path_len =
            self.branch_path.len() + common_prefix_len + self.maybe_parent_nibble();
        self.branch_path = new_child_path.slice_unchecked(0, branch_path_len);

        // Push the new branch onto the `branch_stack`. We do not yet set the `state_mask` bit of
        // the new child; whatever actually pushes the child onto the `child_stack` is expected to
        // do that.
        self.branch_stack.push(ProofTrieBranch {
            ext_len: common_prefix_len as u8,
            state_mask: TrieMask::new(1 << first_child_nibble),
            masks: None,
        });

        trace!(
            target: TRACE_TARGET,
            ?new_child_path,
            ?common_prefix_len,
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
    fn pop_branch<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
    ) -> Result<(), StateProofError> {
        trace!(
            target: TRACE_TARGET,
            branch = ?self.branch_stack.last(),
            branch_path = ?self.branch_path,
            child_stack_len = ?self.child_stack.len(),
            "pop_branch: called",
        );

        // Ensure the final child on the child stack has been committed, as this method expects all
        // children of the branch to have been committed.
        self.commit_last_child(targets)?;

        let mut rlp_nodes_buf = self.take_rlp_nodes_buf();
        let branch = self.branch_stack.pop().expect("branch_stack cannot be empty");

        // Take the branch's children off the stack, using the state mask to determine how many
        // there are.
        let num_children = branch.state_mask.count_ones() as usize;
        debug_assert!(num_children > 1, "A branch must have at least two children");
        debug_assert!(
            self.child_stack.len() >= num_children,
            "Stack is missing necessary children ({num_children:?})"
        );

        // Collect children into RlpNode Vec. Children are in lexicographic order.
        for child in self.child_stack.drain(self.child_stack.len() - num_children..) {
            let child_rlp_node = match child {
                ProofTrieBranchChild::RlpNode(rlp_node) => rlp_node,
                uncommitted_child => {
                    // Convert uncommitted child (not retained for proof) to RlpNode now.
                    self.rlp_encode_buf.clear();
                    let (rlp_node, freed_buf) =
                        uncommitted_child.into_rlp(&mut self.rlp_encode_buf)?;
                    if let Some(buf) = freed_buf {
                        self.rlp_nodes_bufs.push(buf);
                    }
                    rlp_node
                }
            };
            rlp_nodes_buf.push(child_rlp_node);
        }

        debug_assert_eq!(
            rlp_nodes_buf.len(),
            branch.state_mask.count_ones() as usize,
            "children length must match number of bits set in state_mask"
        );

        // Calculate the short key of the parent extension (if the branch has a parent extension).
        // It's important to calculate this short key prior to modifying the `branch_path`.
        let short_key = trim_nibbles_prefix(
            &self.branch_path,
            self.branch_path.len() - branch.ext_len as usize,
        );

        // Wrap the `BranchNode` so it can be pushed onto the child stack.
        let mut branch_as_child = ProofTrieBranchChild::Branch {
            node: BranchNode::new(rlp_nodes_buf, branch.state_mask),
            masks: branch.masks,
        };

        // If there is an extension then encode the branch as an `RlpNode` and use it to construct
        // the extension in its place
        if !short_key.is_empty() {
            let branch_rlp_node = self.commit_child(targets, self.branch_path, branch_as_child)?;
            branch_as_child = ProofTrieBranchChild::Extension { short_key, child: branch_rlp_node };
        };

        self.child_stack.push(branch_as_child);

        // Update the branch_path. If this branch is the only branch then only its extension needs
        // to be trimmed, otherwise we also need to remove its nibble from its parent.
        let new_path_len =
            self.branch_path.len() - branch.ext_len as usize - self.maybe_parent_nibble();

        debug_assert!(self.branch_path.len() >= new_path_len);
        self.branch_path = self.branch_path.slice_unchecked(0, new_path_len);

        Ok(())
    }

    /// Adds a single leaf for a key to the stack, possibly collapsing an existing branch and/or
    /// creating a new one depending on the path of the key.
    fn push_leaf<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        key: Nibbles,
        val: VE::DeferredEncoder,
    ) -> Result<(), StateProofError> {
        loop {
            trace!(
                target: TRACE_TARGET,
                ?key,
                branch_stack_len = ?self.branch_stack.len(),
                branch_path = ?self.branch_path,
                child_stack_len = ?self.child_stack.len(),
                "push_leaf: loop",
            );

            // Get the `state_mask` of the branch currently being built. If there are no branches
            // on the stack then it means either the trie is empty or only a single leaf has been
            // added previously.
            let curr_branch_state_mask = match self.branch_stack.last() {
                Some(curr_branch) => curr_branch.state_mask,
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
                    let (nibble, short_key) = self.push_new_branch(key);
                    self.push_new_leaf(targets, nibble, short_key, val)?;
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
                self.pop_branch(targets)?;
                continue
            }

            // If the current branch is a prefix of the new key then the leaf is a child of the
            // branch. If the branch doesn't have the leaf's nibble set then the leaf can be added
            // directly, otherwise a new branch must be created in-between this branch and that
            // existing child.
            let nibble = key.get_unchecked(common_prefix_len);
            if curr_branch_state_mask.is_bit_set(nibble) {
                // Push a new branch which splits the short key of the existing child at this
                // nibble.
                let (nibble, short_key) = self.push_new_branch(key);
                // Push the new leaf onto the new branch.
                self.push_new_leaf(targets, nibble, short_key, val)?;
            } else {
                let short_key = key.slice_unchecked(common_prefix_len + 1, key.len());
                self.push_new_leaf(targets, nibble, short_key, val)?;
            }

            return Ok(())
        }
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
        hashed_cursor_current: &mut Option<(Nibbles, VE::DeferredEncoder)>,
        lower_bound: Nibbles,
        upper_bound: Option<Nibbles>,
    ) -> Result<(), StateProofError> {
        // A helper closure for mapping entries returned from the `hashed_cursor`, converting the
        // key to Nibbles and immediately creating the DeferredValueEncoder so that encoding of the
        // leaf value can begin ASAP.
        let mut map_hashed_cursor_entry = |(key_b256, val): (B256, _)| {
            debug_assert_eq!(key_b256.len(), 32);
            // SAFETY: key is a B256 and so is exactly 32-bytes.
            let key = unsafe { Nibbles::unpack_unchecked(key_b256.as_slice()) };
            let val = value_encoder.deferred_encoder(key_b256, val);
            (key, val)
        };

        // If the cursor hasn't been used, or the last iterated key is prior to this range's
        // key range, then seek forward to at least the first key.
        if hashed_cursor_current.as_ref().is_none_or(|(key, _)| key < &lower_bound) {
            trace!(
                target: TRACE_TARGET,
                current=?hashed_cursor_current.as_ref().map(|(k, _)| k),
                "Seeking hashed cursor to meet lower bound",
            );

            let lower_key = B256::right_padding_from(&lower_bound.pack());
            *hashed_cursor_current =
                self.hashed_cursor.seek(lower_key)?.map(&mut map_hashed_cursor_entry);
        }

        // Loop over all keys in the range, calling `push_leaf` on each.
        while let Some((key, _)) = hashed_cursor_current.as_ref() &&
            upper_bound.is_none_or(|upper_bound| key < &upper_bound)
        {
            let (key, val) =
                core::mem::take(hashed_cursor_current).expect("while-let checks for Some");
            self.push_leaf(targets, key, val)?;
            *hashed_cursor_current = self.hashed_cursor.next()?.map(&mut map_hashed_cursor_entry);
        }

        trace!(target: TRACE_TARGET, "No further keys within range");
        Ok(())
    }

    /// Constructs and returns a new [`ProofTrieBranch`] based on an existing [`BranchNodeCompact`].
    #[inline]
    const fn new_from_cached_branch(
        cached_branch: &BranchNodeCompact,
        ext_len: u8,
    ) -> ProofTrieBranch {
        ProofTrieBranch {
            ext_len,
            state_mask: TrieMask::new(0),
            masks: Some(BranchNodeMasks {
                tree_mask: cached_branch.tree_mask,
                hash_mask: cached_branch.hash_mask,
            }),
        }
    }

    /// Pushes a new branch onto the `branch_stack` which is based on a cached branch obtained via
    /// the trie cursor.
    ///
    /// If there is already a child at the top branch of `branch_stack` occupying this new branch's
    /// nibble then that child will have its short-key split with another new branch, and this
    /// cached branch will be a child of that splitting branch.
    fn push_cached_branch<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        cached_path: Nibbles,
        cached_branch: &BranchNodeCompact,
    ) -> Result<(), StateProofError> {
        debug_assert!(
            cached_path.starts_with(&self.branch_path),
            "push_cached_branch called with path {cached_path:?} which is not a child of current branch {:?}",
            self.branch_path,
        );

        let parent_branch = self.branch_stack.last();

        // If both stacks are empty then there were no leaves before this cached branch, push it and
        // be done; the extension of the branch will be its full path.
        if self.child_stack.is_empty() && parent_branch.is_none() {
            self.branch_path = cached_path;
            self.branch_stack
                .push(Self::new_from_cached_branch(cached_branch, cached_path.len() as u8));
            return Ok(())
        }

        // Get the nibble which should be set in the parent branch's `state_mask` for this new
        // branch.
        let cached_branch_nibble = cached_path.get_unchecked(self.branch_path.len());

        // We calculate the `ext_len` of the new branch, and potentially update its nibble if a new
        // parent branch is inserted here, based on the state of the parent branch.
        let (cached_branch_nibble, ext_len) = if parent_branch
            .is_none_or(|parent_branch| parent_branch.state_mask.is_bit_set(cached_branch_nibble))
        {
            // If the `child_stack` is not empty but the `branch_stack` is then it implies that
            // there must be a leaf or extension at the root of the trie whose short-key will get
            // split by a new branch, which will become the parent of both that leaf/extension and
            // this new branch.
            //
            // Similarly, if there is a branch on the `branch_stack` but its `state_mask` bit for
            // this new branch is already set, then there must be a leaf/extension with a short-key
            // to be split.
            debug_assert!(!self
                .child_stack
                .last()
                .expect("already checked for emptiness")
                .short_key()
                .is_empty());

            // Split that leaf/extension's short key with a new branch.
            let (nibble, short_key) = self.push_new_branch(cached_path);
            (nibble, short_key.len())
        } else {
            // If there is a parent branch but its `state_mask` bit for this branch is not set
            // then we can simply calculate the `ext_len` based on the difference of each, minus
            // 1 to account for the nibble in the `state_mask`.
            (cached_branch_nibble, cached_path.len() - self.branch_path.len() - 1)
        };

        // `commit_last_child` relies on the last set bit of the parent branch's `state_mask` to
        // determine the path of the last child on the `child_stack`. Since we are about to
        // change that mask we need to commit that last child first.
        self.commit_last_child(targets)?;

        // When pushing a new branch we need to set its child nibble in the `state_mask` of
        // its parent, if there is one.
        if let Some(parent_branch) = self.branch_stack.last_mut() {
            parent_branch.state_mask.set_bit(cached_branch_nibble);
        }

        // Finally update the `branch_path` and push the new branch.
        self.branch_path = cached_path;
        self.branch_stack.push(Self::new_from_cached_branch(cached_branch, ext_len as u8));

        trace!(
            target: TRACE_TARGET,
            branch=?self.branch_stack.last(),
            branch_path=?self.branch_path,
            "Pushed cached branch",
        );

        Ok(())
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
        sub_trie_prefix: &Nibbles,
        uncalculated_lower_bound: &Option<Nibbles>,
    ) -> Result<PopCachedBranchOutcome, StateProofError> {
        // If there is a branch on top of the stack we use that.
        if let Some(cached) = self.cached_branch_stack.pop() {
            return Ok(PopCachedBranchOutcome::Popped(cached));
        }

        // There is no cached branch on the stack. It's possible that another one exists
        // farther on in the trie, but we perform some checks first to prevent unnecessary
        // attempts to find it.

        // If the `uncalculated_lower_bound` is None it indicates that there can be no more
        // leaf data, so similarly there can be no more branches.
        let Some(uncalculated_lower_bound) = uncalculated_lower_bound else {
            return Ok(PopCachedBranchOutcome::Exhausted)
        };

        // If [`TrieCursorState::path`] returns None it means that the cursor has been
        // exhausted, so there can be no more cached data.
        let Some(mut trie_cursor_path) = trie_cursor_state.path() else {
            return Ok(PopCachedBranchOutcome::Exhausted)
        };

        // If the trie cursor is seeked to a branch whose leaves have already been processed
        // then we can't use it, instead we seek forward and try again.
        if trie_cursor_path < uncalculated_lower_bound {
            *trie_cursor_state =
                TrieCursorState::seeked(self.trie_cursor.seek(*uncalculated_lower_bound)?);

            // Having just seeked forward we need to check if the cursor is now exhausted,
            // extracting the new path at the same time.
            if let Some(new_trie_cursor_path) = trie_cursor_state.path() {
                trie_cursor_path = new_trie_cursor_path
            } else {
                return Ok(PopCachedBranchOutcome::Exhausted)
            };
        }

        // If the trie cursor has exceeded the sub-trie then we consider it to be exhausted.
        if !trie_cursor_path.starts_with(sub_trie_prefix) {
            return Ok(PopCachedBranchOutcome::Exhausted)
        }

        // At this point we can be sure that the cursor is in an `Available` state. We know for
        // sure it's not `Exhausted` because of the calls to `path` above, and we know it's not
        // `Taken` because we push all taken branches onto the `cached_branch_stack`, and the
        // stack is empty.
        //
        // We will use this `Available` cached branch as our next branch.
        let cached = trie_cursor_state.take();
        trace!(target: TRACE_TARGET, cached=?cached, "Pushed next trie node onto cached_branch_stack");

        // If the calculated range is not caught up to the next cached branch it means there
        // are portions of the trie prior to that branch which may need to be calculated;
        // return the uncalculated range up to that branch to make that happen.
        //
        // If the next cached branch's path is all zeros then we can skip this catch-up step,
        // because there cannot be any keys prior to that range.
        let cached_path = &cached.0;
        if uncalculated_lower_bound < cached_path && !PATH_ALL_ZEROS.starts_with(cached_path) {
            let range = (*uncalculated_lower_bound, Some(*cached_path));
            trace!(target: TRACE_TARGET, ?range, "Returning key range to calculate in order to catch up to cached branch");

            // Push the cached branch onto the stack so it's available once the leaf range is done
            // being calculated.
            self.cached_branch_stack.push(cached);

            return Ok(PopCachedBranchOutcome::CalculateLeaves(range));
        }

        Ok(PopCachedBranchOutcome::Popped(cached))
    }

    /// Accepts the current state of both hashed and trie cursors, and determines the next range of
    /// hashed keys which need to be processed using [`Self::push_leaf`].
    ///
    /// This method will use cached branch node data from the trie cursor to skip over all possible
    /// ranges of keys, to reduce computation as much as possible.
    ///
    /// # Returns
    ///
    /// - `None`: No more data to process, finish computation
    ///
    /// - `Some(lower, None)`: Indicates to call `push_leaf` on all keys starting at `lower`, with
    ///   no upper bound. This method won't be called again after this.
    ///
    /// - `Some(lower, Some(upper))`: Indicates to call `push_leaf` on all keys starting at `lower`,
    ///   up to but excluding `upper`, and then call this method once done.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    fn next_uncached_key_range<'a>(
        &mut self,
        targets: &mut Option<TargetsCursor<'a>>,
        trie_cursor_state: &mut TrieCursorState,
        sub_trie_prefix: &Nibbles,
        sub_trie_upper_bound: Option<&Nibbles>,
        mut uncalculated_lower_bound: Option<Nibbles>,
    ) -> Result<Option<(Nibbles, Option<Nibbles>)>, StateProofError> {
        // Pop any under-construction branches that are now complete.
        // All trie data prior to the current cached branch, if any, has been computed. Any branches
        // which were under-construction previously, and which are not on the same path as this
        // cached branch, can be assumed to be completed; they will not have any further keys added.
        // to them.
        if let Some(cached_path) = self.cached_branch_stack.last().map(|kv| kv.0) {
            while !cached_path.starts_with(&self.branch_path) {
                self.pop_branch(targets)?;
            }
        }

        loop {
            // Pop the currently cached branch node.
            //
            // NOTE we pop off the `cached_branch_stack` because cloning the `BranchNodeCompact`
            // means cloning an Arc, which incurs synchronization overhead. We have to be sure to
            // push the cached branch back onto the stack once done.
            let (cached_path, cached_branch) = match self.try_pop_cached_branch(
                trie_cursor_state,
                sub_trie_prefix,
                &uncalculated_lower_bound,
            )? {
                PopCachedBranchOutcome::Popped(cached) => cached,
                PopCachedBranchOutcome::Exhausted => {
                    // If cached branches are exhausted it's possible that there is still an
                    // unbounded range of leaves to be processed. `uncalculated_lower_bound` is
                    // used to return that range.
                    trace!(target: TRACE_TARGET, ?uncalculated_lower_bound, "Exhausted cached trie nodes");
                    return Ok(uncalculated_lower_bound
                        .map(|lower| (lower, sub_trie_upper_bound.copied())));
                }
                PopCachedBranchOutcome::CalculateLeaves(range) => {
                    return Ok(Some(range));
                }
            };

            trace!(
                target: TRACE_TARGET,
                branch_path = ?self.branch_path,
                branch_state_mask = ?self.branch_stack.last().map(|b| b.state_mask),
                ?cached_path,
                cached_branch_state_mask = ?cached_branch.state_mask,
                cached_branch_hash_mask = ?cached_branch.hash_mask,
                "loop",
            );

            // Since we've popped all branches which don't start with cached_path, branch_path at
            // this point must be equal to or shorter than cached_path.
            debug_assert!(
                self.branch_path.len() < cached_path.len() || self.branch_path == cached_path,
                "branch_path {:?} is different-or-longer-than cached_path {cached_path:?}",
                self.branch_path
            );

            // If the branch_path != cached_path it means the branch_stack is either empty, or the
            // top branch is the parent of this cached branch. Either way we push a branch
            // corresponding to the cached one onto the stack, so we can begin constructing it.
            if self.branch_path != cached_path {
                self.push_cached_branch(targets, cached_path, &cached_branch)?;
            }

            // At this point the top of the branch stack is the same branch which was found in the
            // cache.
            let curr_branch =
                self.branch_stack.last().expect("top of branch_stack corresponds to cached branch");

            let cached_state_mask = cached_branch.state_mask.get();
            let curr_state_mask = curr_branch.state_mask.get();

            // Determine all child nibbles which are set in the cached branch but not the
            // under-construction branch.
            let next_child_nibbles = curr_state_mask ^ cached_state_mask;
            debug_assert_eq!(
                cached_state_mask | next_child_nibbles, cached_state_mask,
                "curr_branch has state_mask bits set which aren't set on cached_branch. curr_branch:{:?}",
                curr_state_mask,
            );

            // If there are no further children to construct for this branch then pop it off both
            // stacks and loop using the parent branch.
            if next_child_nibbles == 0 {
                trace!(
                    target: TRACE_TARGET,
                    path=?cached_path,
                    ?curr_branch,
                    ?cached_branch,
                    "No further children, popping branch",
                );
                self.pop_branch(targets)?;

                // no need to pop from `cached_branch_stack`, the current cached branch is already
                // popped (see note at the top of the loop).

                // The just-popped branch is completely processed; we know there can be no more keys
                // with that prefix. Set the lower bound which can be returned from this method to
                // be the next possible prefix, if any.
                uncalculated_lower_bound = increment_and_strip_trailing_zeros(&cached_path);

                continue
            }

            // Determine the next nibble of the branch which has not yet been constructed, and
            // determine the child's full path.
            let child_nibble = next_child_nibbles.trailing_zeros() as u8;
            let child_path = self.child_path_at(child_nibble);

            // If the `hash_mask` bit is set for the next child it means the child's hash is cached
            // in the `cached_branch`. We can use that instead of re-calculating the hash of the
            // entire sub-trie.
            //
            // If the child needs to be retained for a proof then we should not use the cached
            // hash, and instead continue on to calculate its node manually.
            if cached_branch.hash_mask.is_bit_set(child_nibble) {
                // Commit the last child. We do this here for two reasons:
                // - `commit_last_child` will check if the last child needs to be retained. We need
                //   to check that before the subsequent `should_retain` call here to prevent
                //   `targets` from being moved beyond the last child before it is checked.
                // - If we do end up using the cached hash value, then we will need to commit the
                //   last child before pushing a new one onto the stack anyway.
                self.commit_last_child(targets)?;

                if !self.should_retain(targets, &child_path, false) {
                    // Pull this child's hash out of the cached branch node. To get the hash's index
                    // we first need to calculate the mask of which cached hashes have already been
                    // used by this branch (if any). The number of set bits in that mask will be the
                    // index of the next hash in the array to use.
                    let curr_hashed_used_mask = cached_branch.hash_mask.get() & curr_state_mask;
                    let hash_idx = curr_hashed_used_mask.count_ones() as usize;
                    let hash = cached_branch.hashes[hash_idx];

                    trace!(
                        target: TRACE_TARGET,
                        ?child_path,
                        ?hash_idx,
                        ?hash,
                        "Using cached hash for child",
                    );

                    self.child_stack.push(ProofTrieBranchChild::RlpNode(RlpNode::word_rlp(&hash)));
                    self.branch_stack
                        .last_mut()
                        .expect("already asserted there is a last branch")
                        .state_mask
                        .set_bit(child_nibble);

                    // Update the `uncalculated_lower_bound` to indicate that the child whose bit
                    // was just set is completely processed.
                    uncalculated_lower_bound = increment_and_strip_trailing_zeros(&child_path);

                    // Push the current cached branch back onto the stack before looping.
                    self.cached_branch_stack.push((cached_path, cached_branch));

                    continue
                }
            }

            // We now want to check if there is a cached branch node at this child. The cached
            // branch node may be the node at this child directly, or this child may be an
            // extension and the cached branch is the child of that extension.

            // All trie nodes prior to `child_path` will not be modified further, so we can seek the
            // trie cursor to the next cached node at-or-after `child_path`.
            if trie_cursor_state.path().is_some_and(|path| path < &child_path) {
                trace!(target: TRACE_TARGET, ?child_path, "Seeking trie cursor to child path");
                *trie_cursor_state = TrieCursorState::seeked(self.trie_cursor.seek(child_path)?);
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
                self.cached_branch_stack.push(trie_cursor_state.take());
                continue;
            }

            // There is no cached data for the sub-trie at this child, we must recalculate the
            // sub-trie root (this child) using the leaves. Return the range of keys based on the
            // child path.
            let child_path_upper = increment_and_strip_trailing_zeros(&child_path);
            trace!(
                target: TRACE_TARGET,
                lower=?child_path,
                upper=?child_path_upper,
                "Returning sub-trie's key range to calculate",
            );

            // Push the current cached branch back onto the stack before returning.
            self.cached_branch_stack.push((cached_path, cached_branch));

            return Ok(Some((child_path, child_path_upper)));
        }
    }

    /// Calculates trie nodes and retains proofs for targeted nodes within a sub-trie. The
    /// sub-trie's bounds are denoted by the `lower_bound` and `upper_bound` arguments,
    /// `upper_bound` is exclusive, None indicates unbounded.
    #[instrument(
        target = TRACE_TARGET,
        level = "trace",
        skip_all,
        fields(prefix=?sub_trie_targets.prefix),
    )]
    fn proof_subtrie<'a>(
        &mut self,
        value_encoder: &mut VE,
        trie_cursor_state: &mut TrieCursorState,
        hashed_cursor_current: &mut Option<(Nibbles, VE::DeferredEncoder)>,
        sub_trie_targets: SubTrieTargets<'a>,
    ) -> Result<(), StateProofError> {
        let sub_trie_upper_bound = sub_trie_targets.upper_bound();

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
        // cursor to have already been seeked. If it's not yet seeked, or seeked to a prior node,
        // then we seek it to the prefix (the first possible node) to initialize it.
        if trie_cursor_state.before(&sub_trie_targets.prefix) {
            trace!(target: TRACE_TARGET, "Doing initial seek of trie cursor");
            *trie_cursor_state =
                TrieCursorState::seeked(self.trie_cursor.seek(sub_trie_targets.prefix)?);
        }

        // `uncalculated_lower_bound` tracks the lower bound of node paths which have yet to be
        // visited, either via the hashed key cursor (`calculate_key_range`) or trie cursor
        // (`next_uncached_key_range`). If/when this becomes None then there are no further nodes
        // which could exist.
        let mut uncalculated_lower_bound = Some(sub_trie_targets.prefix);

        trace!(target: TRACE_TARGET, "Starting loop");
        loop {
            // Save the previous lower bound to detect forward progress.
            let prev_uncalculated_lower_bound = uncalculated_lower_bound;

            // Determine the range of keys of the overall trie which need to be re-computed.
            let Some((calc_lower_bound, calc_upper_bound)) = self.next_uncached_key_range(
                &mut targets,
                trie_cursor_state,
                &sub_trie_targets.prefix,
                sub_trie_upper_bound.as_ref(),
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
                     prev_lower={prev_lower:?}, calc_upper={calc_upper_bound:?}, prefix={:?}",
                    sub_trie_targets.prefix,
                );
                error!(target: TRACE_TARGET, "{msg}");
                return Err(StateProofError::TrieInconsistency(msg));
            }

            // Calculate the trie for that range of keys
            self.calculate_key_range(
                value_encoder,
                &mut targets,
                hashed_cursor_current,
                calc_lower_bound,
                calc_upper_bound,
            )?;

            // Once outside `calculate_key_range`, `hashed_cursor_current` will be at the first key
            // after the range.
            //
            // If the `hashed_cursor_current` is None (exhausted), or not within the range of the
            // sub-trie, then there are no more keys at all, meaning the trie couldn't possibly have
            // more data and we should complete computation.
            if hashed_cursor_current
                .as_ref()
                .is_none_or(|(key, _)| !key.starts_with(&sub_trie_targets.prefix))
            {
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

        // We always pop the root node off of the `child_stack` in order to empty it, however we
        // might not want to retain the node unless the `SubTrieTargets` indicates it.
        trace!(
            target: TRACE_TARGET,
            retain_root = ?sub_trie_targets.retain_root,
            child_stack_empty = self.child_stack.is_empty(),
            "Maybe retaining root",
        );
        match (sub_trie_targets.retain_root, self.child_stack.pop()) {
            (false, _) => {
                // Whether the root node is exists or not, we don't want it.
            }
            (true, None) => {
                // If `child_stack` is empty it means there was no keys at all, retain an empty
                // root node.
                self.retained_proofs.push(ProofTrieNode {
                    path: Nibbles::new(), // root path
                    node: TrieNode::EmptyRoot,
                    masks: None,
                });
            }
            (true, Some(root_node)) => {
                // Encode and retain the root node.
                self.rlp_encode_buf.clear();
                let root_node =
                    root_node.into_proof_trie_node(Nibbles::new(), &mut self.rlp_encode_buf)?;
                self.retained_proofs.push(root_node);
            }
        }

        Ok(())
    }

    /// Internal implementation of proof calculation. Assumes both cursors have already been reset.
    /// See docs on [`Self::proof`] for expected behavior.
    fn proof_inner(
        &mut self,
        value_encoder: &mut VE,
        targets: &mut [Target],
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
        // If there are no targets then nothing could be returned, return early.
        if targets.is_empty() {
            trace!(target: TRACE_TARGET, "Empty targets, returning");
            return Ok(Vec::new())
        }

        // Initialize the variables which track the state of the two cursors. Both indicate the
        // cursors are unseeked.
        let mut trie_cursor_state = TrieCursorState::unseeked();
        let mut hashed_cursor_current: Option<(Nibbles, VE::DeferredEncoder)> = None;

        // Divide targets into chunks, each chunk corresponding to a different sub-trie within the
        // overall trie, and handle all proofs within that sub-trie.
        for sub_trie_targets in iter_sub_trie_targets(targets) {
            self.proof_subtrie(
                value_encoder,
                &mut trie_cursor_state,
                &mut hashed_cursor_current,
                sub_trie_targets,
            )?;
        }

        trace!(
            target: TRACE_TARGET,
            retained_proofs_len = ?self.retained_proofs.len(),
            "proof_inner: returning",
        );
        Ok(core::mem::take(&mut self.retained_proofs))
    }

    /// Generate a proof for the given targets.
    ///
    /// Given a set of [`Target`]s, returns nodes whose paths are a prefix of any target. The
    /// returned nodes will be sorted depth-first by path.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    pub fn proof(
        &mut self,
        value_encoder: &mut VE,
        targets: &mut [Target],
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
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
        proof_nodes: &[ProofTrieNode],
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
    pub fn root_node(&mut self, value_encoder: &mut VE) -> Result<ProofTrieNode, StateProofError> {
        // Initialize the variables which track the state of the two cursors. Both indicate the
        // cursors are unseeked.
        let mut trie_cursor_state = TrieCursorState::unseeked();
        let mut hashed_cursor_current: Option<(Nibbles, VE::DeferredEncoder)> = None;

        static EMPTY_TARGETS: [Target; 0] = [];
        let sub_trie_targets =
            SubTrieTargets { prefix: Nibbles::new(), targets: &EMPTY_TARGETS, retain_root: true };

        self.proof_subtrie(
            value_encoder,
            &mut trie_cursor_state,
            &mut hashed_cursor_current,
            sub_trie_targets,
        )?;

        // proof_subtrie will retain the root node if retain_proof is true, regardless of if there
        // are any targets.
        let mut proofs = core::mem::take(&mut self.retained_proofs);
        trace!(
            target: TRACE_TARGET,
            proofs_len = ?proofs.len(),
            "root_node: extracting root",
        );

        // The root node is at the empty path - it must exist since retain_root is true. Otherwise
        // targets was empty, so there should be no other retained proofs.
        debug_assert_eq!(
            proofs.len(), 1,
            "prefix is empty, retain_root is true, and targets is empty, so there must be only the root node"
        );

        // Find and remove the root node (node at empty path)
        let root_node = proofs.pop().expect("prefix is empty, retain_root is true, and targets is empty, so there must be only the root node");

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
    /// Given a set of [`Target`]s, returns nodes whose paths are a prefix of any target. The
    /// returned nodes will be sorted depth-first by path.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the targets are not sorted lexicographically.
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self, targets))]
    pub fn storage_proof(
        &mut self,
        hashed_address: B256,
        targets: &mut [Target],
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        // Shortcut: check if storage is empty
        if self.hashed_cursor.is_storage_empty()? {
            // Return a single EmptyRoot node at the root path
            return Ok(vec![ProofTrieNode {
                path: Nibbles::default(),
                node: TrieNode::EmptyRoot,
                masks: None,
            }])
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
    ) -> Result<ProofTrieNode, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        if self.hashed_cursor.is_storage_empty()? {
            return Ok(ProofTrieNode {
                path: Nibbles::default(),
                node: TrieNode::EmptyRoot,
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

/// Helper type wrapping a slice of [`Target`]s, primarily used to iterate through targets in
/// [`ProofCalculator::should_retain`].
///
/// It is assumed that the underlying slice is never empty, and that the iterator is never
/// exhausted.
struct TargetsCursor<'a> {
    targets: &'a [Target],
    i: usize,
}

impl<'a> TargetsCursor<'a> {
    /// Wraps a slice of [`Target`]s with the `TargetsCursor`.
    ///
    /// # Panics
    ///
    /// Will panic in debug mode if called with an empty slice.
    fn new(targets: &'a [Target]) -> Self {
        debug_assert!(!targets.is_empty());
        Self { targets, i: 0 }
    }

    /// Returns the current and next [`Target`] that the cursor is pointed at.
    fn current(&self) -> (&'a Target, Option<&'a Target>) {
        (&self.targets[self.i], self.targets.get(self.i + 1))
    }

    /// Iterates the cursor forward.
    ///
    /// # Panics
    ///
    /// Will panic if the cursor is exhausted.
    fn next(&mut self) -> (&'a Target, Option<&'a Target>) {
        self.i += 1;
        debug_assert!(self.i < self.targets.len());
        self.current()
    }

    // Iterate forwards over the slice, starting from the [`Target`] after the current.
    fn skip_iter(&self) -> impl Iterator<Item = &'a Target> {
        self.targets[self.i + 1..].iter()
    }

    /// Iterated backwards over the slice, starting from the [`Target`] previous to the current.
    fn rev_iter(&self) -> impl Iterator<Item = &'a Target> {
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
    /// Cursor has been exhausted.
    Exhausted,
}

impl TrieCursorState {
    /// Creates a [`Self::Unseeked`] based on an entry returned from the cursor itself.
    const fn unseeked() -> Self {
        Self::Unseeked
    }

    /// Creates a [`Self`] based on an entry returned from the cursor itself.
    fn seeked(entry: Option<(Nibbles, BranchNodeCompact)>) -> Self {
        entry.map_or(Self::Exhausted, |(path, node)| Self::Available(path, node))
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
            Self::Exhausted => None,
        }
    }

    /// Returns true if the cursor is unseeked, or is seeked to a node prior to the given one.
    fn before(&self, path: &Nibbles) -> bool {
        match self {
            Self::Unseeked => true,
            Self::Available(seeked_to, _) | Self::Taken(seeked_to) => path < seeked_to,
            Self::Exhausted => false,
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

/// Increments the nibbles and strips any trailing zeros.
///
/// This function wraps `Nibbles::increment` and when it returns a value with trailing zeros,
/// it strips those zeros using bit manipulation on the underlying U256.
fn increment_and_strip_trailing_zeros(nibbles: &Nibbles) -> Option<Nibbles> {
    let mut result = nibbles.increment()?;

    // If result is empty, just return it
    if result.is_empty() {
        return Some(result);
    }

    // Get access to the underlying U256 to detect trailing zeros
    let uint_val = *result.as_mut_uint_unchecked();
    let non_zero_prefix_len = 64 - (uint_val.trailing_zeros() / 4);
    result.truncate(non_zero_prefix_len);

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{
            mock::MockHashedCursorFactory, HashedCursorFactory, HashedCursorMetricsCache,
            InstrumentedHashedCursor,
        },
        proof::Proof,
        trie_cursor::{
            depth_first, mock::MockTrieCursorFactory, InstrumentedTrieCursor, TrieCursorFactory,
            TrieCursorMetricsCache,
        },
    };
    use alloy_primitives::map::{B256Map, B256Set};
    use alloy_rlp::Decodable;
    use itertools::Itertools;
    use reth_primitives_traits::Account;
    use reth_trie_common::{
        updates::{StorageTrieUpdates, TrieUpdates},
        HashedPostState, MultiProofTargets, TrieNode,
    };

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
        /// The expected state root, calculated by `StateRoot`
        expected_root: B256,
    }

    impl ProofTestHarness {
        /// Creates a new test harness from a `HashedPostState`.
        ///
        /// The `HashedPostState` is used to populate the mock hashed cursor factory directly.
        /// The trie cursor factory is initialized from `TrieUpdates` generated by `StateRoot`.
        fn new(post_state: HashedPostState) -> Self {
            // Create empty trie cursor factory to serve as the initial state for StateRoot
            // Ensure that there's a storage trie dataset for every account, to make
            // `MockTrieCursorFactory` happy.
            let storage_tries: B256Map<_> = post_state
                .accounts
                .keys()
                .copied()
                .map(|addr| (addr, StorageTrieUpdates::default()))
                .collect();

            let empty_trie_cursor_factory = MockTrieCursorFactory::from_trie_updates(TrieUpdates {
                storage_tries: storage_tries.clone(),
                ..Default::default()
            });

            // Create mock hashed cursor factory from the post state
            let hashed_cursor_factory = MockHashedCursorFactory::from_hashed_post_state(post_state);

            // Generate TrieUpdates using StateRoot
            let (expected_root, mut trie_updates) =
                crate::StateRoot::new(empty_trie_cursor_factory, hashed_cursor_factory.clone())
                    .root_with_updates()
                    .expect("StateRoot should succeed");

            // Continue using empty storage tries for each account, to keep `MockTrieCursorFactory`
            // happy.
            trie_updates.storage_tries = storage_tries;

            // Initialize trie cursor factory from the generated TrieUpdates
            let trie_cursor_factory = MockTrieCursorFactory::from_trie_updates(trie_updates);

            Self { trie_cursor_factory, hashed_cursor_factory, expected_root }
        }

        /// Asserts that `ProofCalculator` and legacy `Proof` produce equivalent results for account
        /// proofs.
        ///
        /// This method calls both implementations with the given account targets and compares
        /// the results.
        fn assert_proof(
            &self,
            targets: impl IntoIterator<Item = Target>,
        ) -> Result<(), StateProofError> {
            let targets_vec = targets.into_iter().collect::<Vec<_>>();

            // Convert Target keys to MultiProofTargets for legacy implementation
            // For account-only proofs, each account maps to an empty storage set
            // Legacy implementation only uses the keys, not the prefix
            let legacy_targets = targets_vec
                .iter()
                .map(|target| (B256::from_slice(&target.key.pack()), B256Set::default()))
                .collect::<MultiProofTargets>();

            // Create ProofCalculator (proof_v2) with account cursors
            let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;
            let hashed_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;

            // Collect metrics for cursors
            let mut trie_cursor_metrics = TrieCursorMetricsCache::default();
            let trie_cursor = InstrumentedTrieCursor::new(trie_cursor, &mut trie_cursor_metrics);
            let mut hashed_cursor_metrics = HashedCursorMetricsCache::default();
            let hashed_cursor =
                InstrumentedHashedCursor::new(hashed_cursor, &mut hashed_cursor_metrics);

            // Call ProofCalculator::proof with account targets
            let mut value_encoder = SyncAccountValueEncoder::new(
                self.trie_cursor_factory.clone(),
                self.hashed_cursor_factory.clone(),
            );
            let mut proof_calculator = ProofCalculator::new(trie_cursor, hashed_cursor);
            let proof_v2_result =
                proof_calculator.proof(&mut value_encoder, &mut targets_vec.clone())?;

            // Output metrics
            trace!(target: TRACE_TARGET, ?trie_cursor_metrics, "V2 trie cursor metrics");
            trace!(target: TRACE_TARGET, ?hashed_cursor_metrics, "V2 hashed cursor metrics");

            // Call Proof::multiproof (legacy implementation)
            let proof_legacy_result =
                Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                    .with_branch_node_masks(true)
                    .multiproof(legacy_targets)?;

            // Helper function to check if a node path matches at least one target
            let node_matches_target = |node_path: &Nibbles| -> bool {
                targets_vec.iter().any(|target| {
                    // Node path must be a prefix of the target's key
                    target.key.starts_with(node_path) &&
                    // Node path must be at least `min_len` long
                    node_path.len() >= target.min_len as usize
                })
            };

            // Decode and sort legacy proof nodes, filtering to only those that match at least one
            // target
            let proof_legacy_nodes = proof_legacy_result
                .account_subtree
                .iter()
                .filter(|(path, _)| node_matches_target(path))
                .map(|(path, node_enc)| {
                    let mut buf = node_enc.as_ref();
                    let node = TrieNode::decode(&mut buf)
                        .expect("legacy implementation should not produce malformed proof nodes");

                    // The legacy proof calculator will calculate masks for the root node, even
                    // though we never store the root node so the masks for it aren't really valid.
                    let masks = if path.is_empty() {
                        None
                    } else {
                        proof_legacy_result.branch_node_masks.get(path).copied()
                    };

                    ProofTrieNode { path: *path, node, masks }
                })
                .sorted_by(|a, b| depth_first::cmp(&a.path, &b.path))
                .collect::<Vec<_>>();

            // Basic comparison: both should succeed and produce identical results
            pretty_assertions::assert_eq!(proof_legacy_nodes, proof_v2_result);

            // Also test root_node - get a fresh calculator and verify it returns the root node
            // that hashes to the expected root
            let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;
            let hashed_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
            let mut value_encoder = SyncAccountValueEncoder::new(
                self.trie_cursor_factory.clone(),
                self.hashed_cursor_factory.clone(),
            );
            let mut proof_calculator = ProofCalculator::new(trie_cursor, hashed_cursor);
            let root_node = proof_calculator.root_node(&mut value_encoder)?;

            // The root node should be at the empty path
            assert!(root_node.path.is_empty(), "root_node should return node at empty path");

            // The hash of the root node should match the expected root from legacy StateRoot
            let root_hash = proof_calculator
                .compute_root_hash(&[root_node])?
                .expect("root_node returns a node at empty path");
            pretty_assertions::assert_eq!(self.expected_root, root_hash);

            Ok(())
        }
    }

    mod proptest_tests {
        use super::*;
        use alloy_primitives::{map::B256Map, U256};
        use proptest::prelude::*;
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
            prop::collection::vec((any::<[u8; 32]>(), account_strategy()), 0..=100).prop_map(
                |accounts| {
                    let account_map = accounts
                        .into_iter()
                        .map(|(addr_bytes, account)| (B256::from(addr_bytes), Some(account)))
                        .collect::<B256Map<_>>();

                    HashedPostState { accounts: account_map, ..Default::default() }
                },
            )
        }

        /// Generate a strategy for proof targets that are 80% from the `HashedPostState` accounts
        /// and 20% random keys. Each target has a random `min_len` of 0..16.
        fn proof_targets_strategy(account_keys: Vec<B256>) -> impl Strategy<Value = Vec<Target>> {
            let num_accounts = account_keys.len();

            // Generate between 0 and (num_accounts + 5) targets
            let target_count = 0..=(num_accounts + 5);

            target_count.prop_flat_map(move |count| {
                let account_keys = account_keys.clone();
                prop::collection::vec(
                    (
                        prop::bool::weighted(0.8).prop_flat_map(move |from_accounts| {
                            if from_accounts && !account_keys.is_empty() {
                                // 80% chance: pick from existing account keys
                                prop::sample::select(account_keys.clone()).boxed()
                            } else {
                                // 20% chance: generate random B256
                                any::<[u8; 32]>().prop_map(B256::from).boxed()
                            }
                        }),
                        0u8..16u8, // Random min_len from 0 to 15
                    )
                        .prop_map(|(key, min_len)| Target::new(key).with_min_len(min_len)),
                    count,
                )
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(8000))]
            #[test]
            /// Tests that ProofCalculator produces valid proofs for randomly generated
            /// HashedPostState with proof targets.
            ///
            /// This test:
            /// - Generates random accounts in a HashedPostState
            /// - Generates proof targets: 80% from existing account keys, 20% random
            /// - Creates a test harness with the generated state
            /// - Calls assert_proof with the generated targets
            /// - Verifies both ProofCalculator and legacy Proof produce equivalent results
            fn proptest_proof_with_targets(
                (post_state, targets) in hashed_post_state_strategy()
                    .prop_flat_map(|post_state| {
                        let mut account_keys: Vec<B256> = post_state.accounts.keys().copied().collect();
                        // Sort to ensure deterministic order when using PROPTEST_RNG_SEED
                        account_keys.sort_unstable();
                        let targets_strategy = proof_targets_strategy(account_keys);
                        (Just(post_state), targets_strategy)
                    })
            ) {
                reth_tracing::init_test_tracing();
                let harness = ProofTestHarness::new(post_state);

                harness.assert_proof(targets).expect("Proof generation failed");
            }
        }
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

        // Generate random HashedPostState.
        let mut post_state = HashedPostState::default();
        for _ in 0..10240 {
            let hashed_addr = rand_b256();
            let account = Account { bytecode_hash: Some(hashed_addr), ..Default::default() };
            post_state.accounts.insert(hashed_addr, Some(account));
        }

        // Collect targets; partially from real keys, partially random keys which probably won't
        // exist.
        let mut targets = post_state.accounts.keys().copied().collect::<Vec<_>>();
        for _ in 0..post_state.accounts.len() / 5 {
            targets.push(rand_b256());
        }
        targets.sort();

        // Create test harness
        let harness = ProofTestHarness::new(post_state);

        // Assert the proof (convert B256 to Target with no min_len for this test)
        harness
            .assert_proof(targets.into_iter().map(Target::new))
            .expect("Proof generation failed");
    }

    #[test]
    fn test_increment_and_strip_trailing_zeros() {
        let test_cases: Vec<(Nibbles, Option<Nibbles>)> = vec![
            // Basic increment without trailing zeros
            (Nibbles::from_nibbles([0x1, 0x2, 0x3]), Some(Nibbles::from_nibbles([0x1, 0x2, 0x4]))),
            // Increment with trailing zeros - should be stripped
            (Nibbles::from_nibbles([0x0, 0x0, 0xF]), Some(Nibbles::from_nibbles([0x0, 0x1]))),
            (Nibbles::from_nibbles([0x0, 0xF, 0xF]), Some(Nibbles::from_nibbles([0x1]))),
            // Overflow case
            (Nibbles::from_nibbles([0xF, 0xF, 0xF]), None),
            // Empty nibbles
            (Nibbles::new(), None),
            // Single nibble
            (Nibbles::from_nibbles([0x5]), Some(Nibbles::from_nibbles([0x6]))),
            // All Fs except last - results in trailing zeros after increment
            (Nibbles::from_nibbles([0xE, 0xF, 0xF]), Some(Nibbles::from_nibbles([0xF]))),
        ];

        for (input, expected) in test_cases {
            let result = increment_and_strip_trailing_zeros(&input);
            assert_eq!(result, expected, "Failed for input: {:?}", input);
        }
    }

    #[test]
    fn test_failing_proptest_case_0() {
        use alloy_primitives::{hex, map::B256Map};

        reth_tracing::init_test_tracing();

        // Helper function to create B256 from hex string
        let b256 = |s: &str| B256::from_slice(&hex::decode(s).unwrap());

        // Create the HashedPostState from test case input
        let mut accounts = B256Map::default();

        // Define all account data from test case input
        let account_data = [
            (
                "9f3a475db85ff1f5b5e82d8614ee4afc670d27aefb9a43da0bd863a54acf1fe6",
                8396790837504194281u64,
                9224366602005816983u64,
                "103c5b0538f4e37944321a30f5cb1f7005d2ee70998106f34f36d7adb838c789",
            ),
            (
                "c736258fdfd23d73ec4c5e54b8c3b58e26726b361d438ef48670f028286b70ca",
                9193115115482903760u64,
                4515164289866465875u64,
                "9f24ef3ab0b4893b0ec38d0e9b00f239da072ccf093b0b24f1ea1f99547abe55",
            ),
            (
                "780a3476520090f97e847181aee17515c5ea30b7607775103df16d2b6611a87a",
                8404772182417755681u64,
                16639574952778823617u64,
                "214b12bee666ce8c64c6bbbcfafa0c3e55b4b05a8724ec4182b9a6caa774c56d",
            ),
            (
                "23ebfa849308a5d02c3048040217cd1f4b71fb01a9b54dafe541284ebec2bcce",
                17978809803974566048u64,
                11093542035392742776u64,
                "5384dfda8f1935d98e463c00a96960ff24e4d4893ec21e5ece0d272df33ac7e9",
            ),
            (
                "348e476c24fac841b11d358431b4526db09edc9f39906e0ac8809886a04f3c5a",
                9422945522568453583u64,
                9737072818780682487u64,
                "79f8f25b2cbb7485c5c7b627917c0f562f012d3d7ddd486212c90fbea0cf686e",
            ),
            (
                "830536ee6c8f780a1cd760457345b79fc09476018a59cf3e8fd427a793d99633",
                16497625187081138489u64,
                15143978245385012455u64,
                "00ede4000cc2a16fca7e930761aaf30d1fddcc3803f0009d6a0742b4ee519342",
            ),
            (
                "806c74b024b2fe81f077ea93d2936c489689f7fe024febc3a0fb71a8a9f22fbc",
                8103477314050566918u64,
                1383893458340561723u64,
                "690ed176136174c4f0cc442e6dcbcf6e7b577e30fc052430b6060f97af1f8e85",
            ),
            (
                "b903d962ffc520877f14e1e8328160e5b22f8086b0f7e9cba7a373a8376028a0",
                12972727566246296372u64,
                1130659127924527352u64,
                "cadf1f09d8e6a0d945a58ccd2ff36e2ae99f8146f02be96873e84bef0462d64a",
            ),
            (
                "d36a16afff0097e06b2c28bd795b889265e2ceff9a086173113fbeb6f7a9bc42",
                15682404502571860137u64,
                2025886798818635036u64,
                "c2cee70663e9ff1b521e2e1602e88723da52ccdc7a69e370cde9595af435e654",
            ),
            (
                "f3e8461cba0b84f5b81f8ca63d0456cb567e701ec1d6e77b1a03624c5018389b",
                5663749586038550112u64,
                7681243595728002238u64,
                "072c547c3ab9744bcd2ed9dbd813bd62866a673f4ca5d46939b65e9507be0e70",
            ),
            (
                "40b71840b6f43a493b32f4aa755e02d572012392fd582c81a513a169447e194c",
                518207789203399614u64,
                317311275468085815u64,
                "85541d48471bf639c2574600a9b637338c49729ba9e741f157cc6ebaae139da0",
            ),
            (
                "3f77cd91ceb7d335dd2527c29e79aaf94f14141438740051eb0163d86c35bcc9",
                16227517944662106096u64,
                12646193931088343779u64,
                "54999911d82dd63d526429275115fa98f6a560bc2d8e00be24962e91e38d7182",
            ),
            (
                "5cd903814ba84daa6956572411cd1bf4d48a8e230003d28cc3f942697bf8debb",
                5096288383163945009u64,
                17919982845103509853u64,
                "6a53c812e713f1bfe6bf21954f291140c60ec3f2ef353ecdae5dc7b263a37282",
            ),
            (
                "23f3602c95fd98d7fbe48a326ae1549030a2c7574099432cce5b458182f16bf2",
                11136020130962086191u64,
                12045219101880183180u64,
                "ce53fb9b108a3ee90db8469e44948ba3263ca8d8a0d92a076c9516f9a3d30bd1",
            ),
            (
                "be86489b3594a9da83e04a9ff81c8d68d528b8b9d31f3942d1c5856a4a8c5af7",
                16293506537092575994u64,
                536238712429663046u64,
                "a2af0607ade21241386ecfb3780aa90514f43595941daeff8dd599c203cde30a",
            ),
            (
                "97bcd85ee5d6033bdf86397e8b26f711912948a7298114be27ca5499ea99725f",
                3086656672041156193u64,
                8667446575959669532u64,
                "0474377538684a991ffc9b41f970b48e65eda9e07c292e60861258ef87d45272",
            ),
            (
                "40065932e6c70eb907e4f2a89ec772f5382ca90a49ef44c4ae21155b9decdcc0",
                17152529399128063686u64,
                3643450822628960860u64,
                "d5f6198c64c797f455f5b44062bb136734f508f9cdd02d8d69d24100ac8d6252",
            ),
            (
                "c136436c2db6b2ebd14985e2c883e73c6d8fd95ace54bfefae9eeca47b7da800",
                727585093455815585u64,
                521742371554431881u64,
                "3dfad04a6eb46d175b63e96943c7d636c56d61063277e25557aace95820432da",
            ),
            (
                "9ea50348595593788645394eb041ac4f75ee4d6a4840b9cf1ed304e895060791",
                8654829249939415079u64,
                15623358443672184321u64,
                "61bb0d6ffcd5b32d0ee34a3b7dfb1c495888059be02b255dd1fa3be02fa1ddbd",
            ),
            (
                "5abc714353ad6abda44a609f9b61f310f5b0a7df55ccf553dc2db3edda18ca17",
                5732104102609402825u64,
                15720007305337585794u64,
                "8b55b7e9c6f54057322c5e0610b33b3137f1fcd46f7d4af1aca797c7b5fff033",
            ),
            (
                "e270b59e6e56100f9e2813f263884ba5f74190a1770dd88cd9603266174e0a6b",
                4728642361690813205u64,
                6762867306120182099u64,
                "5e9aa1ff854504b4bfea4a7f0175866eba04e88e14e57ac08dddc63d6917bf47",
            ),
            (
                "78286294c6fb6823bb8b2b2ddb7a1e71ee64e05c9ba33b0eb8bb6654c64a8259",
                6032052879332640150u64,
                498315069638377858u64,
                "799ef578ffb51a5ec42484e788d6ada4f13f0ff73e1b7b3e6d14d58caae9319a",
            ),
            (
                "af1b85cf284b0cb59a4bfb0f699194bcd6ad4538f27057d9d93dc7a95c1ff32e",
                1647153930670480138u64,
                13109595411418593026u64,
                "429dcdf4748c0047b0dd94f3ad12b5e62bbadf8302525cc5d2aad9c9c746696f",
            ),
            (
                "0152b7a0626771a2518de84c01e52839e7821a655f9dcb9a174d8f52b64b7086",
                3915492299782594412u64,
                9550071871839879785u64,
                "4d5e6ce993dfc9597585ae2b4bacd6d055fefc56ae825666c83e0770e4aa0527",
            ),
            (
                "9ea9b8a4f6bce1dba63290b81f4d1b88dfeac3e244856904a5c9d4086a10271b",
                8824593031424861220u64,
                15831101445348312026u64,
                "a07602b4dd5cba679562061b7c5c0344b2edd6eba36aa97ca57a6fe01ed80a48",
            ),
            (
                "d7b26c2d8f85b74423a57a3da56c61829340f65967791bab849c90b5e1547e7a",
                12723258987146468813u64,
                10714399360315276559u64,
                "3705e57b27d931188c0d2017ab62577355b0cdda4173203478a8562a0cdcae0c",
            ),
            (
                "da354ceca117552482e628937931870a28e9d4416f47a58ee77176d0b760c75b",
                1580954430670112951u64,
                14920857341852745222u64,
                "a13d6b0123daa2e662699ac55a2d0ed1d2e73a02ed00ee5a4dd34db8dea2a37e",
            ),
            (
                "53140d0c8b90b4c3c49e0604879d0dc036e914c4c4f799f1ccae357fef2613e3",
                12521658365236780592u64,
                11630410585145916252u64,
                "46f06ce1435a7a0fd3476bbcffe4aac88c33a7fcf50080270b715d25c93d96d7",
            ),
            (
                "4b1c151815da6f18f27e98890eac1f7d43b80f3386c7c7d15ee0e43a7edfe0a6",
                9575643484508382933u64,
                3471795678079408573u64,
                "a9e6a8fac46c5fc61ae07bddc223e9f105f567ad039d2312a03431d1f24d8b2c",
            ),
            (
                "39436357a2bcd906e58fb88238be2ddb2e43c8a5590332e3aee1d1134a0d0ba4",
                10171391804125392783u64,
                2915644784933705108u64,
                "1d5db03f07137da9d3af85096ed51a4ff64bb476a79bf4294850438867fe3833",
            ),
            (
                "5fbe8d9d6a12b061a94a72436caec331ab1fd4e472c3bb4688215788c5e9bcd9",
                5663512925993713993u64,
                18170240962605758111u64,
                "bd5d601cbcb47bd84d410bafec72f2270fceb1ed2ed11499a1e218a9f89a9f7f",
            ),
            (
                "f2e29a909dd31b38e9b92b2b2d214e822ebddb26183cd077d4009773854ab099",
                7512894577556564068u64,
                15905517369556068583u64,
                "a36e66ce11eca7900248c518e12c6c08d659d609f4cbd98468292de7adf780f2",
            ),
            (
                "3eb82e6d6e964ca56b50cc54bdd55bb470c67a4932aba48d27d175d1be2542aa",
                12645567232869276853u64,
                8416544129280224452u64,
                "d177f246a45cc76d39a8ee06b32d8c076c986106b9a8e0455a0b41d00fe3cbde",
            ),
            (
                "c903731014f6a5b4b45174ef5f9d5a2895a19d1308292f25aa323fda88acc938",
                5989992708726918818u64,
                17462460601463602125u64,
                "01241c61ad1c8adc27e5a1096ab6c643af0fbb6e2818ef77272b70e5c3624abc",
            ),
            (
                "ef46410ab47113a78c27e100ed1b476f82a8789012bd95a047a4b23385596f53",
                11884362385049322305u64,
                619908411193297508u64,
                "e9b4c929e26077ac1fd5a771ea5badc7e9ddb58a20a2a797389c63b3dd3df00d",
            ),
            (
                "be336bc6722bb787d542f4ef8ecb6f46a449557ca7b69b8668b6fed19dfa73b7",
                11490216175357680195u64,
                13136528075688203375u64,
                "31bfd807f92e6d5dc5c534e9ad0cb29d00c6f0ae7d7b5f1e65f8e683de0bce59",
            ),
            (
                "39599e5828a8f102b8a6808103ae7df29b838fe739d8b73f72f8f0d282ca5a47",
                6957481657451522177u64,
                4196708540027060724u64,
                "968a12d79704b313471ece148cb4e26b8b11620db2a9ee6da0f5dc200801f555",
            ),
            (
                "acd99530bb14ca9a7fac3df8eebfd8cdd234b0f6f7c3893a20bc159a4fd54df5",
                9792913946138032169u64,
                9219321015500590384u64,
                "db45a98128770a329c82c904ceee21d3917f6072b8bd260e46218f65656c964c",
            ),
            (
                "453b80a0b11f237011c57630034ed46888ad96f4300a58aea24c0fe4a5472f68",
                14407140330317286994u64,
                5783848199433986576u64,
                "b8cded0b4efd6bf2282a4f8b3c353f74821714f84df9a6ab25131edc7fdad00f",
            ),
            (
                "23e464d1e9b413a4a6b378cee3a0405ec6ccbb4d418372d1b42d3fde558d48d1",
                1190974500816796805u64,
                1621159728666344828u64,
                "d677f41d273754da3ab8080b605ae07a7193c9f35f6318b809e42a1fdf594be3",
            ),
            (
                "d0e590648dec459aca50edf44251627bab5a36029a0c748b1ddf86b7b887425b",
                4807164391931567365u64,
                4256042233199858200u64,
                "a8677de59ab856516a03663730af54c55a79169346c3d958b564e5ee35d8622b",
            ),
            (
                "72387dbaaaf2c39175d8c067558b869ba7bdc6234bc63ee97a53fea1d988ff39",
                5046042574093452325u64,
                3088471405044806123u64,
                "83c226621506b07073936aec3c87a8e2ef34dd42e504adc2bbab39ede49aa77f",
            ),
            (
                "de6874ca2b9dd8b4347c25d32b882a2a7c127b127d6c5e00d073ab3853339d0e",
                6112730660331874479u64,
                10943246617310133253u64,
                "a0c96a69e5ab3e3fe1a1a2fd0e5e68035ff3c7b2985e4e6b8407d4c377600c6f",
            ),
            (
                "b0d8689e08b983e578d6a0c136b76952497087ee144369af653a0a1b231eeb28",
                15612408165265483596u64,
                13112504741499957010u64,
                "4fc49edeff215f1d54dfd2e60a14a3de2abecbe845db2148c7aee32c65f3c91c",
            ),
            (
                "29d7fb6b714cbdd1be95c4a268cef7f544329642ae05fab26dc251bbc773085e",
                17509162400681223655u64,
                5075629528173950353u64,
                "781ecb560ef8cf0bcfa96b8d12075f4cf87ad52d69dfb2c72801206eded135bd",
            ),
            (
                "85dbf7074c93a4e39b67cc504b35351ee16c1fab437a7fb9e5d9320be1d9c13c",
                17692199403267011109u64,
                7069378948726478427u64,
                "a3ff0d8dee5aa0214460f5b03a70bd76ef00ac8c07f07c0b3d82c9c57e4c72a9",
            ),
            (
                "7bd5a9f3126b4a681afac9a177c6ff7f3dd80d8d7fd5a821a705221c96975ded",
                17807965607151214145u64,
                5562549152802999850u64,
                "dbc3861943b7372e49698b1c5b0e4255b7c93e9fa2c13d6a4405172ab0db9a5b",
            ),
            (
                "496d13d45dbe7eb02fee23c914ac9fefdf86cf5c937c520719fc6a31b3fcf8d9",
                13446203348342334214u64,
                332407928246785326u64,
                "d2d73f15fcdc12adce25b911aa4551dcf900e225761e254eb6392cbd414e389c",
            ),
            (
                "b2f0a0127fc74a35dec5515b1c7eb8a3833ca99925049c47cd109ec94678e6c5",
                9683373807753869342u64,
                7570798132195583433u64,
                "e704110433e5ab17858c5fbe4f1b6d692942d5f5981cac68372d06066bee97fe",
            ),
            (
                "d5f65171b17d7720411905ef138e84b9d1f459e2b248521c449f1781aafd675e",
                10088287051097617949u64,
                185695341767856973u64,
                "8d784c4171e242af4187f30510cd298106b7e68cd3088444a055cb1f3893ba28",
            ),
            (
                "7dcbec5c20fbf1d69665d4b9cdc450fea2d0098e78084bce0a864fea4ba016b0",
                13908816056510478374u64,
                17793990636863600193u64,
                "18e9026372d91e116faf813ce3ba9d7fadef2bb3b779be6efeba8a4ecd9e1f38",
            ),
            (
                "d4f772f4bf1cfa4dad4b55962b50900da8657a4961dabbdf0664f3cd42d368f8",
                16438076732493217366u64,
                18419670900047275588u64,
                "b9fd16b16b3a8fab4d9c47f452d9ce4aad530edeb06ee6830589078db2f79382",
            ),
            (
                "2d009535f82b1813ce2ca7236ceae7864c1e4d3644a1acd02656919ef1aa55d0",
                10206924399607440433u64,
                3986996560633257271u64,
                "db49e225bd427768599a7c06d7aee432121fa3179505f9ee8c717f51c7fa8c54",
            ),
            (
                "b1d7a292df12e505e7433c7e850e9efc81a8931b65f3354a66402894b6d5ba76",
                8215550459234533539u64,
                10241096845089693964u64,
                "5567813b312cb811909a01d14ee8f7ec4d239198ea2d37243123e1de2317e1af",
            ),
            (
                "85120d6f43ea9258accf6a87e49cd5461d9b3735a4dc623f9fbcc669cbdd1ce6",
                17566770568845511328u64,
                8686605711223432099u64,
                "e163f4fcd17acf5714ee48278732808601e861cd4c4c24326cd24431aab1d0ce",
            ),
            (
                "48fe4c22080c6e702f7af0e97fb5354c1c14ff4616c6fc4ac8a4491d4b9b3473",
                14371024664575587429u64,
                15149464181957728462u64,
                "061dec7af4b41bdd056306a8b13b71d574a49a4595884b1a77674f5150d4509d",
            ),
            (
                "29d14b014fa3cabbb3b4808e751e81f571de6d0e727cae627318a5fd82fef517",
                9612395342616083334u64,
                3700617080099093094u64,
                "f7b33a2d2784441f77f0cc1c87930e79bea3332a921269b500e81d823108561c",
            ),
        ];

        // Insert all accounts
        for (addr, nonce, balance, code_hash) in &account_data {
            accounts.insert(
                b256(addr),
                Some(Account {
                    nonce: *nonce,
                    balance: U256::from(*balance),
                    bytecode_hash: Some(b256(code_hash)),
                }),
            );
        }

        let post_state = HashedPostState { accounts, storages: Default::default() };

        // Create test harness
        let harness = ProofTestHarness::new(post_state);

        // Create targets from test case input - these are Nibbles in hex form
        let targets = vec![
            Target::new(b256("0153000000000000000000000000000000000000000000000000000000000000"))
                .with_min_len(2),
            Target::new(b256("0000000000000000000000000000000000000000000000000000000000000000"))
                .with_min_len(2),
            Target::new(b256("2300000000000000000000000000000000000000000000000000000000000000"))
                .with_min_len(2),
        ];

        // Test proof generation
        harness.assert_proof(targets).expect("Proof generation failed");
    }
}
