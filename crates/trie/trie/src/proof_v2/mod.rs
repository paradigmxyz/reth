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
    BranchNodeMasks, BranchNodeRef, BranchNodeV2, Nibbles, ProofTrieNodeV2, ProofV2Target, RlpNode,
    TrieNodeV2,
};
use std::cmp::Ordering;
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
    retained_proofs: Vec<ProofTrieNodeV2>,
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
                |ProofTrieNodeV2 { path: last_retained_path, .. }| {
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
            if lower.key_nibbles.starts_with(path) {
                return !check_min_len ||
                    (path.len() >= lower.min_len as usize ||
                        targets
                            .skip_iter()
                            .take_while(|target| target.key_nibbles.starts_with(path))
                            .any(|target| path.len() >= target.min_len as usize) ||
                        targets
                            .rev_iter()
                            .take_while(|target| target.key_nibbles.starts_with(path))
                            .any(|target| path.len() >= target.min_len as usize))
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

            // Convert to `ProofTrieNodeV2`, which will be what is retained.
            //
            // If this node is a branch then its `rlp_nodes_buf` will be taken and not returned to
            // the `rlp_nodes_bufs` free-list.
            self.rlp_encode_buf.clear();
            let proof_node = child.into_proof_trie_node(child_path, &mut self.rlp_encode_buf)?;

            // Use the `ProofTrieNodeV2` to encode the `RlpNode`, and then push it onto retained
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

        // Compute hash for the branch node if it has a parent extension.
        let rlp_node = if short_key.is_empty() {
            None
        } else {
            self.rlp_encode_buf.clear();
            BranchNodeRef::new(&rlp_nodes_buf, branch.state_mask).encode(&mut self.rlp_encode_buf);
            Some(RlpNode::from_rlp(&self.rlp_encode_buf))
        };

        // Wrap the `BranchNodeV2` so it can be pushed onto the child stack.
        let branch_as_child = ProofTrieBranchChild::Branch {
            node: BranchNodeV2::new(short_key, rlp_nodes_buf, branch.state_mask, rlp_node),
            masks: branch.masks,
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
            let key = Nibbles::unpack_array(key_b256.as_ref());
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

            let cached_state_mask = cached_branch.state_mask;
            let curr_state_mask = curr_branch.state_mask;

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
            if next_child_nibbles.is_empty() {
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
                uncalculated_lower_bound = cached_path.next_without_prefix();

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
                    let curr_hashed_used_mask = cached_branch.hash_mask & curr_state_mask;
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
                    uncalculated_lower_bound = child_path.next_without_prefix();

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
            let child_path_upper = child_path.next_without_prefix();
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
                self.retained_proofs.push(ProofTrieNodeV2 {
                    path: Nibbles::new(), // root path
                    node: TrieNodeV2::EmptyRoot,
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
        let mut hashed_cursor_current: Option<(Nibbles, VE::DeferredEncoder)> = None;

        // Divide targets into chunks, each chunk corresponding to a different sub-trie within the
        // overall trie, and handle all proofs within that sub-trie.
        for sub_trie_targets in iter_sub_trie_targets(targets) {
            if let Err(err) = self.proof_subtrie(
                value_encoder,
                &mut trie_cursor_state,
                &mut hashed_cursor_current,
                sub_trie_targets,
            ) {
                self.clear_computation_state();
                return Err(err);
            }
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
        // Initialize the variables which track the state of the two cursors. Both indicate the
        // cursors are unseeked.
        let mut trie_cursor_state = TrieCursorState::unseeked();
        let mut hashed_cursor_current: Option<(Nibbles, VE::DeferredEncoder)> = None;

        static EMPTY_TARGETS: [ProofV2Target; 0] = [];
        let sub_trie_targets =
            SubTrieTargets { prefix: Nibbles::new(), targets: &EMPTY_TARGETS, retain_root: true };

        if let Err(err) = self.proof_subtrie(
            value_encoder,
            &mut trie_cursor_state,
            &mut hashed_cursor_current,
            sub_trie_targets,
        ) {
            self.clear_computation_state();
            return Err(err);
        }

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
            // Return a single EmptyRoot node at the root path
            return Ok(vec![ProofTrieNodeV2 {
                path: Nibbles::default(),
                node: TrieNodeV2::EmptyRoot,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::HashedCursorFactory,
        proof::StorageProof as LegacyStorageProof,
        test_utils::TrieTestHarness,
        trie_cursor::{depth_first, TrieCursorFactory},
    };
    use alloy_primitives::map::B256Set;
    use alloy_rlp::Decodable;
    use alloy_trie::proof::AddedRemovedKeys;
    use itertools::Itertools;
    use reth_trie_common::{ProofTrieNode, TrieNode};
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

            // Convert ProofV2Target keys to B256Set for legacy implementation
            let legacy_targets = targets_vec
                .iter()
                .map(|target| B256::from_slice(&target.key_nibbles.pack()))
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

            // Helper function to check if a node path matches at least one target
            let node_matches_target = |node_path: &Nibbles| -> bool {
                targets_vec.iter().any(|target| {
                    target.key_nibbles.starts_with(node_path) &&
                        node_path.len() >= target.min_len as usize
                })
            };

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
            let proof_legacy_nodes_v2 = convert_legacy_proofs_to_v2(&proof_legacy_nodes);

            // Filter both results to only keep nodes which match a target. The v2
            // storage_proof returns an EmptyRoot node even when there are no targets, so
            // both sides need the same filtering.
            let proof_legacy_nodes_v2 = proof_legacy_nodes_v2
                .into_iter()
                .filter(|ProofTrieNodeV2 { path, .. }| node_matches_target(path))
                .collect::<Vec<_>>();

            let proof_v2_result = proof_v2_result
                .into_iter()
                .filter(|ProofTrieNodeV2 { path, .. }| node_matches_target(path))
                .collect::<Vec<_>>();

            pretty_assertions::assert_eq!(proof_legacy_nodes_v2, proof_v2_result);

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
        proof_calculator.branch_stack.push(ProofTrieBranch {
            ext_len: 2,
            state_mask: TrieMask::new(0b1111),
            masks: None,
        });
        proof_calculator.branch_stack.push(ProofTrieBranch {
            ext_len: 0,
            state_mask: TrieMask::new(0b11),
            masks: None,
        });
        proof_calculator
            .child_stack
            .push(ProofTrieBranchChild::RlpNode(RlpNode::word_rlp(&B256::ZERO)));
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
        /// and 20% random keys. Each target has a random `min_len` of 0..16.
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
                        .prop_map(|(key, min_len)| ProofV2Target::new(key).with_min_len(min_len)),
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
}
