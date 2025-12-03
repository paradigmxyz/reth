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
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{BranchNode, Nibbles, ProofTrieNode, RlpNode, TrieMasks, TrieNode};
use std::{cmp::Ordering, iter::Peekable};
use tracing::{instrument, trace};

mod value;
pub use value::*;

mod node;
use node::*;

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

/// Used to track the state of the trie cursor, allowing us to differentiate between a branch having
/// been taken (used as a cached branch) and the cursor having been exhausted.
#[derive(Debug)]
enum TrieCursorState {
    /// Cursor is seeked to this path and the node has not been used yet.
    Available(Nibbles, BranchNodeCompact),
    /// Cursor is seeked to this path, but the node has been used.
    Taken(Nibbles),
    /// Cursor has been exhausted.
    Exhausted,
}

impl TrieCursorState {
    /// Creates a [`Self`] based on an entry returned from the cursor itself.
    fn new(entry: Option<(Nibbles, BranchNodeCompact)>) -> Self {
        entry.map_or(Self::Exhausted, |(path, node)| Self::Available(path, node))
    }

    /// Returns the path the cursor is seeked to, or None if it's exhausted.
    const fn path(&self) -> Option<&Nibbles> {
        match self {
            Self::Available(path, _) | Self::Taken(path) => Some(path),
            Self::Exhausted => None,
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
    /// duration of a proof calculation, but occasionally we will lose one when when a branch
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
            child_stack: Vec::<_>::new(),
            cached_branch_stack: Vec::<_>::with_capacity(64),
            retained_proofs: Vec::<_>::new(),
            rlp_nodes_bufs: Vec::<_>::new(),
            rlp_encode_buf: Vec::<_>::with_capacity(RLP_ENCODE_BUF_SIZE),
        }
    }
}

/// Helper type for the [`Iterator`] used to pass targets in from the caller.
type TargetsIter<I> = Peekable<WindowIter<I>>;

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

    /// Returns true if the proof of a node at the given path should be retained.
    /// A node is retained if its path is a prefix of any target.
    /// This may move the
    /// `targets` iterator forward if the given path comes after the current target.
    ///
    /// This method takes advantage of the [`WindowIter`] component of [`TargetsIter`] to only check
    /// a single target at a time. The [`WindowIter`] allows us to look at a current target and the
    /// next target simultaneously, forming an end-exclusive range.
    ///
    /// ```text
    /// * Given targets: [ 0x012, 0x045, 0x678 ]
    /// * targets.next() returns:
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
    ///     - targets.peek() returns (0x012, Some(0x045))
    ///
    /// * 0x04 comes _after_ 0x045 in depth-first order, so (0x012..0x045) does not contain 0x04.
    ///
    /// * targets.next() is called.
    ///
    /// * targets.peek() now returns (0x045, Some(0x678)). This does contain 0x04.
    ///
    /// * 0x04 is a prefix of 0x045, and so is retained.
    /// ```
    ///
    /// Because paths in the trie are visited in depth-first order, it's imperative that targets are
    /// given in depth-first order as well. If the targets were generated off of B256s, which is
    /// the common-case, then this is equivalent to lexicographical order.
    fn should_retain(
        &self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
        path: &Nibbles,
    ) -> bool {
        trace!(target: TRACE_TARGET, ?path, target = ?targets.peek(), "should_retain: called");
        debug_assert!(self.retained_proofs.last().is_none_or(
                |ProofTrieNode { path: last_retained_path, .. }| {
                    depth_first::cmp(path, last_retained_path) == Ordering::Greater
                }
            ),
            "should_retain called with path {path:?} which is not after previously retained node {:?} in depth-first order",
            self.retained_proofs.last().map(|n| n.path),
        );

        let &(mut lower, mut upper) = targets.peek().expect("targets is never exhausted");

        loop {
            // If the node in question is a prefix of the target then we retain
            if lower.starts_with(path) {
                return true
            }

            // If the path isn't in the current range then iterate forward until it is (or until
            // there is no upper bound, indicating unbounded).
            if upper.is_some_and(|upper| depth_first::cmp(path, &upper) != Ordering::Less) {
                targets.next();
                trace!(target: TRACE_TARGET, target = ?targets.peek(), "upper target <= path, next target");
                let &(l, u) = targets.peek().expect("targets is never exhausted");
                (lower, upper) = (l, u);
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
    fn commit_child(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
        child_path: Nibbles,
        child: ProofTrieBranchChild<VE::DeferredEncoder>,
    ) -> Result<RlpNode, StateProofError> {
        // If the child is already an `RlpNode` then there is nothing to do.
        if let ProofTrieBranchChild::RlpNode(rlp_node) = child {
            return Ok(rlp_node)
        }

        // If we should retain the child then do so.
        if self.should_retain(targets, &child_path) {
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
    /// empty.
    fn last_child_path(&self) -> Nibbles {
        // If there is no branch under construction then the top child must be the root child.
        let Some(branch) = self.branch_stack.last() else {
            return Nibbles::new();
        };

        self.child_path_at(Self::highest_set_nibble(branch.state_mask))
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
    fn commit_last_child(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
    ) -> Result<(), StateProofError> {
        let Some(child) = self.child_stack.pop() else { return Ok(()) };

        // If the child is already an `RlpNode` then there is nothing to do, push it back on with no
        // changes.
        if let ProofTrieBranchChild::RlpNode(_) = child {
            self.child_stack.push(child);
            return Ok(())
        }

        let child_path = self.last_child_path();
        // TODO theoretically `commit_child` only needs to convert to an `RlpNode` if it's going to
        // retain the proof, otherwise we could leave the child as-is on the stack and convert it
        // when popping the branch, giving more time to the DeferredEncoder to do async work.
        let child_rlp_node = self.commit_child(targets, child_path, child)?;

        // Replace the child on the stack
        self.child_stack.push(ProofTrieBranchChild::RlpNode(child_rlp_node));
        Ok(())
    }

    /// Creates a new leaf node on a branch, setting its `state_mask` bit and pushing the leaf onto
    /// the `child_stack`.
    ///
    /// # Panics
    ///
    /// - If `branch_stack` is empty
    /// - If the leaf's nibble is already set in the branch's `state_mask`.
    fn push_new_leaf(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
        leaf_nibble: u8,
        leaf_short_key: Nibbles,
        leaf_val: VE::DeferredEncoder,
    ) -> Result<(), StateProofError> {
        // Before pushing the new leaf onto the `child_stack` we need to commit the previous last
        // child, so that only `child_stack`'s final child is a non-RlpNode.
        self.commit_last_child(targets)?;

        // Once the first child is committed we set the new child's bit on the top branch's
        // `state_mask` and push that child.
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
            masks: TrieMasks::none(),
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
    fn pop_branch(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
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

        // Collect children into an `RlpNode` Vec by committing and pushing each of them.
        for (idx, child) in
            self.child_stack.drain(self.child_stack.len() - num_children..).enumerate()
        {
            let ProofTrieBranchChild::RlpNode(child_rlp_node) = child else {
                panic!(
                    "all branch children must have been committed, found {} at index {idx:?}",
                    std::any::type_name_of_val(&child)
                );
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
    fn push_leaf(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
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
        skip(self, value_encoder, targets, hashed_cursor_current),
    )]
    fn calculate_key_range(
        &mut self,
        value_encoder: &VE,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
        hashed_cursor_current: &mut Option<(Nibbles, VE::DeferredEncoder)>,
        lower_bound: Nibbles,
        upper_bound: Option<Nibbles>,
    ) -> Result<(), StateProofError> {
        // A helper closure for mapping entries returned from the `hashed_cursor`, converting the
        // key to Nibbles and immediately creating the DeferredValueEncoder so that encoding of the
        // leaf value can begin ASAP.
        let map_hashed_cursor_entry = |(key_b256, val): (B256, _)| {
            debug_assert_eq!(key_b256.len(), 32);
            // SAFETY: key is a B256 and so is exactly 32-bytes.
            let key = unsafe { Nibbles::unpack_unchecked(key_b256.as_slice()) };
            let val = value_encoder.deferred_encoder(key_b256, val);
            (key, val)
        };

        // If the cursor hasn't been used, or the last iterated key is prior to this range's
        // key range, then seek forward to at least the first key.
        if hashed_cursor_current.as_ref().is_none_or(|(key, _)| key < &lower_bound) {
            let lower_key = B256::right_padding_from(&lower_bound.pack());
            *hashed_cursor_current =
                self.hashed_cursor.seek(lower_key)?.map(map_hashed_cursor_entry);
        }

        // Loop over all keys in the range, calling `push_leaf` on each.
        while let Some((key, _)) = hashed_cursor_current.as_ref() &&
            upper_bound.is_none_or(|upper_bound| key < &upper_bound)
        {
            let (key, val) =
                core::mem::take(hashed_cursor_current).expect("while-let checks for Some");
            self.push_leaf(targets, key, val)?;
            *hashed_cursor_current = self.hashed_cursor.next()?.map(map_hashed_cursor_entry);
        }

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
            masks: TrieMasks {
                tree_mask: Some(cached_branch.tree_mask),
                hash_mask: Some(cached_branch.hash_mask),
            },
        }
    }

    /// Pushes a new branch onto the `branch_stack` which is based on a cached branch obtained via
    /// the trie cursor.
    ///
    /// If there is already a child at the top branch of `branch_stack` occupying this new branch's
    /// nibble then that child will have its short-key split with another new branch, and this
    /// cached branch will be a child of that splitting branch.
    fn push_cached_branch(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
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
            // We've asserted above that branch_path.len() < cached_path.len(), so this
            // `get_unchecked` is safe.
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
    fn next_uncached_key_range(
        &mut self,
        targets: &mut TargetsIter<impl Iterator<Item = Nibbles>>,
        trie_cursor_state: &mut TrieCursorState,
        hashed_key_current_path: Option<Nibbles>,
    ) -> Result<Option<(Nibbles, Option<Nibbles>)>, StateProofError> {
        // All trie data prior to the current cached branch, if any, has been computed. Any branches
        // which were under-construction previously, and which are not on the same path as this
        // cached branch, can be assumed to be completed; they will not have any further keys added
        // to them.
        if let Some(cached_path) = self.cached_branch_stack.last().map(|kv| kv.0) {
            while !cached_path.starts_with(&self.branch_path) {
                self.pop_branch(targets)?;
            }
        }

        // `lower_bound` will be used to track the lower bound of the range which is returned from
        // this method. If this is None then there are no further keys which need to be processed.
        //
        // This starts off being based off of the hashed cursor's current position, which is the
        // next key which hasn't been processed. If that is None then we start from zero.
        //
        // TODO better name and docs
        let mut lower_bound = Some(hashed_key_current_path.unwrap_or_default());

        loop {
            // Determine the current cached branch node.
            //
            // NOTE we pop off the `cached_branch_stack` because cloning the `BranchNodeCompact`
            // means cloning an Arc, which incurs synchronization overhead. We have to be sure to
            // push the cached branch back onto the stack once done.
            let (cached_path, cached_branch) = match (
                self.cached_branch_stack.pop(),
                &trie_cursor_state,
                lower_bound.as_ref(),
            ) {
                (Some(cached), _, _) => cached,
                (None, TrieCursorState::Exhausted, _) | (_, _, None) => {
                    trace!(target: TRACE_TARGET, "Exhausted cached trie nodes");
                    // If both stack and trie cursor are empty then there are no more cached nodes,
                    // return an open range to indicate that the rest of the trie should be
                    // calculated solely from leaves.
                    //
                    // If the `lower_bound` indicates that there can be no more data then this will
                    // return None to indicate end of computation.
                    return Ok(lower_bound.map(|lower| (lower, None)));
                }
                (None, cursor_state, Some(lower_bound))
                    if cursor_state.path().expect("not exhausted") < lower_bound =>
                {
                    // If `cached_branch_stack` is empty then we want to get a new cached branch
                    // node from the cursor. If the trie cursor is seeked to a branch which has
                    // already been processed then we can't use it, instead we seek forward and try
                    // again.
                    *trie_cursor_state = TrieCursorState::new(self.trie_cursor.seek(*lower_bound)?);
                    continue
                }
                (None, TrieCursorState::Taken(path), _) => {
                    panic!("trie cursor at {path:?} had its node taken, but is >= lower_bound {lower_bound:?}");
                }
                (None, TrieCursorState::Available(_, _), _) => {
                    // If `cached_branch_stack` is empty but there is an available cached branch
                    // from the trie cursor then we consume that branch, pushing it onto the stack.
                    self.cached_branch_stack.push(trie_cursor_state.take());
                    trace!(target: TRACE_TARGET, cached=?self.cached_branch_stack.last(), "Pushed next trie node onto cached_branch_stack");

                    let (cached_path, _) = self.cached_branch_stack.last().expect("just pushed");

                    // The current hashed key indicates the first key after the previous uncached
                    // range, or None if this is the first call to this method.
                    //
                    // If the key is not caught up to the next cached branch it means there are
                    // portions of the trie prior to that branch which need to be computed; return
                    // the uncomputed range up to that branch to make that happen.
                    //
                    // If the next cached branch's path is all zeros then we can skip this catch-up
                    // step, because there cannot be any keys prior to that range.
                    if hashed_key_current_path.is_none_or(|k| &k < cached_path) &&
                        !PATH_ALL_ZEROS.starts_with(cached_path)
                    {
                        let range = lower_bound.map(|lower| (lower, Some(*cached_path)));
                        trace!(target: TRACE_TARGET, ?range, "Returning key range to calculate in order to catch up to cached branch");
                        return Ok(range);
                    }

                    self.cached_branch_stack.pop().expect("just pushed")
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
                lower_bound = cached_path.increment();

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

                if !self.should_retain(targets, &child_path) {
                    // Pull this child's hash out of the cached branch node. To get the hash's index
                    // we first need to calculate the mask of which cached hash's have already been
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

                    // Update the `lower_bound` to indicate that the child whose bit was just set is
                    // completely processed.
                    lower_bound = child_path.increment();

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
                *trie_cursor_state = TrieCursorState::new(self.trie_cursor.seek(child_path)?);
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
            let child_path_upper = child_path.increment();
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

    /// Internal implementation of proof calculation. Assumes both cursors have already been reset.
    /// See docs on [`Self::proof`] for expected behavior.
    fn proof_inner(
        &mut self,
        value_encoder: &VE,
        targets: impl IntoIterator<Item = B256>,
    ) -> Result<Vec<ProofTrieNode>, StateProofError> {
        trace!(target: TRACE_TARGET, "proof_inner: called");

        // In debug builds, verify that targets are sorted
        #[cfg(debug_assertions)]
        let targets = {
            let mut prev: Option<B256> = None;
            targets.into_iter().inspect(move |target| {
                if let Some(prev) = prev {
                    debug_assert!(&prev <= target, "prev:{prev:?} target:{target:?}");
                }
                prev = Some(*target);
            })
        };

        #[cfg(not(debug_assertions))]
        let targets = targets.into_iter();

        // Convert B256 targets into Nibbles.
        let targets = targets.into_iter().map(|key| {
            // SAFETY: key is a B256 and so is exactly 32-bytes.
            unsafe { Nibbles::unpack_unchecked(key.as_slice()) }
        });

        // Wrap targets into a `TargetsIter`.
        let mut targets = WindowIter::new(targets).peekable();

        // If there are no targets then nothing could be returned, return early.
        if targets.peek().is_none() {
            trace!(target: TRACE_TARGET, "Empty targets, returning");
            return Ok(Vec::new())
        }

        // Ensure initial state is cleared. By the end of the method call these should be empty once
        // again.
        debug_assert!(self.branch_stack.is_empty());
        debug_assert!(self.branch_path.is_empty());
        debug_assert!(self.child_stack.is_empty());

        // Initialize the hashed cursor to None to indicate it hasn't been seeked yet.
        let mut hashed_cursor_current: Option<(Nibbles, VE::DeferredEncoder)> = None;

        // Initialize the `trie_cursor_state` with the node closest to root.
        let mut trie_cursor_state = TrieCursorState::new(self.trie_cursor.seek(Nibbles::new())?);

        loop {
            // Determine the range of keys of the overall trie which need to be re-computed.
            let Some((lower_bound, upper_bound)) = self.next_uncached_key_range(
                &mut targets,
                &mut trie_cursor_state,
                hashed_cursor_current.as_ref().map(|kv| kv.0),
            )?
            else {
                // If `next_uncached_key_range` determines that there can be no more keys then
                // complete the computation.
                break;
            };

            // Calculate the trie for that range of keys
            self.calculate_key_range(
                value_encoder,
                &mut targets,
                &mut hashed_cursor_current,
                lower_bound,
                upper_bound,
            )?;

            // Once outside `calculate_key_range`, `hashed_cursor_current` will be at the first key
            // after the range.
            //
            // If the `hashed_cursor_current` is None then there are no more keys at all, meaning
            // the trie couldn't possibly have more data and we should complete computation.
            if hashed_cursor_current.is_none() {
                break;
            }
        }

        // Once there's no more leaves we can pop the remaining branches, if any.
        while !self.branch_stack.is_empty() {
            self.pop_branch(&mut targets)?;
        }

        // At this point the branch stack should be empty. If the child stack is empty it means no
        // keys were ever iterated from the hashed cursor in the first place. Otherwise there should
        // only be a single node left: the root node.
        debug_assert!(self.branch_stack.is_empty());
        debug_assert!(self.branch_path.is_empty());
        debug_assert!(self.child_stack.len() < 2);

        // All targets match the root node, so always retain it. Determine the root node based on
        // the child stack, and push the proof of the root node onto the result stack.
        let root_node = if let Some(node) = self.child_stack.pop() {
            self.rlp_encode_buf.clear();
            node.into_proof_trie_node(Nibbles::new(), &mut self.rlp_encode_buf)?
        } else {
            ProofTrieNode {
                path: Nibbles::new(), // root path
                node: TrieNode::EmptyRoot,
                masks: TrieMasks::none(),
            }
        };
        self.retained_proofs.push(root_node);

        trace!(
            target: TRACE_TARGET,
            retained_proofs_len = ?self.retained_proofs.len(),
            "proof_inner: returning",
        );
        Ok(core::mem::take(&mut self.retained_proofs))
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
        targets: impl IntoIterator<Item = B256>,
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
    pub fn new_storage(trie_cursor: TC, hashed_cursor: HC) -> Self {
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
        targets: impl IntoIterator<Item = B256>,
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

/// `WindowIter` is a wrapper around an [`Iterator`] which allows viewing both previous and current
/// items on every iteration. It is similar to `itertools::tuple_windows`, except that the final
/// item returned will contain the previous item and `None` as the current.
struct WindowIter<I: Iterator> {
    iter: I,
    prev: Option<I::Item>,
}

impl<I: Iterator> WindowIter<I> {
    /// Wraps an iterator with a [`WindowIter`].
    const fn new(iter: I) -> Self {
        Self { iter, prev: None }
    }
}

impl<I: Iterator<Item: Copy>> Iterator for WindowIter<I> {
    /// The iterator returns the previous and current items, respectively. If the underlying
    /// iterator is exhausted then `Some(prev, None)` is returned on the subsequent call to
    /// `WindowIter::next`, and `None` from the call after that.
    type Item = (I::Item, Option<I::Item>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev, self.iter.next()) {
                (None, None) => return None,
                (None, Some(v)) => {
                    self.prev = Some(v);
                }
                (Some(v), next) => {
                    self.prev = next;
                    return Some((v, next))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
        proof::Proof,
        trie_cursor::{depth_first, mock::MockTrieCursorFactory, TrieCursorFactory},
    };
    use alloy_primitives::map::{B256Map, B256Set};
    use alloy_rlp::Decodable;
    use assert_matches::assert_matches;
    use itertools::Itertools;
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
    }

    impl ProofTestHarness {
        /// Creates a new test harness from a `HashedPostState`.
        ///
        /// The `HashedPostState` is used to populate the mock hashed cursor factory directly.
        /// The trie cursor factory is initialized from TrieUpdates generated by StateRoot.
        fn new(post_state: HashedPostState) -> Self {
            trace!(target: TRACE_TARGET, ?post_state, "Creating ProofTestHarness");

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
            let (_root, mut trie_updates) =
                crate::StateRoot::new(empty_trie_cursor_factory, hashed_cursor_factory.clone())
                    .root_with_updates()
                    .expect("StateRoot should succeed");

            // Continue using empty storage tries for each account, to keep `MockTrieCursorFactory`
            // happy.
            trie_updates.storage_tries = storage_tries;

            // Initialize trie cursor factory from the generated TrieUpdates
            let trie_cursor_factory = MockTrieCursorFactory::from_trie_updates(trie_updates);

            Self { trie_cursor_factory, hashed_cursor_factory }
        }

        /// Asserts that `ProofCalculator` and legacy `Proof` produce equivalent results for account
        /// proofs.
        ///
        /// This method calls both implementations with the given account targets and compares
        /// the results.
        fn assert_proof(
            &self,
            targets: impl IntoIterator<Item = B256>,
        ) -> Result<(), StateProofError> {
            let targets_vec = targets.into_iter().sorted().collect::<Vec<_>>();

            // Convert B256 targets to MultiProofTargets for legacy implementation
            // For account-only proofs, each account maps to an empty storage set
            let legacy_targets = targets_vec
                .iter()
                .map(|addr| (*addr, B256Set::default()))
                .collect::<MultiProofTargets>();

            // Create ProofCalculator (proof_v2) with account cursors
            let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;
            let hashed_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;

            // Call ProofCalculator::proof with account targets
            let value_encoder = SyncAccountValueEncoder::new(
                self.trie_cursor_factory.clone(),
                self.hashed_cursor_factory.clone(),
            );
            let mut proof_calculator = ProofCalculator::new(trie_cursor, hashed_cursor);
            let proof_v2_result = proof_calculator.proof(&value_encoder, targets_vec.clone())?;

            // Call Proof::multiproof (legacy implementation)
            let proof_legacy_result =
                Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                    .with_branch_node_masks(true)
                    .multiproof(legacy_targets)?;

            // Decode and sort legacy proof nodes
            let mut proof_legacy_nodes = proof_legacy_result
                .account_subtree
                .iter()
                .map(|(path, node_enc)| {
                    let mut buf = node_enc.as_ref();
                    let node = TrieNode::decode(&mut buf)
                        .expect("legacy implementation should not produce malformed proof nodes");

                    // The legacy proof calculator will calculate masks for the root node, even
                    // though we never store the root node so the masks for it aren't really valid.
                    let masks = if path.is_empty() {
                        TrieMasks::none()
                    } else {
                        TrieMasks {
                            hash_mask: proof_legacy_result
                                .branch_node_hash_masks
                                .get(path)
                                .copied(),
                            tree_mask: proof_legacy_result
                                .branch_node_tree_masks
                                .get(path)
                                .copied(),
                        }
                    };

                    ProofTrieNode { path: *path, node, masks }
                })
                .sorted_by(|a, b| depth_first::cmp(&a.path, &b.path))
                .collect::<Vec<_>>();

            // When no targets are given the legacy implementation will still produce the root node
            // in the proof. This differs from the V2 implementation, which produces nothing when
            // given no targets.
            if targets_vec.is_empty() {
                assert_matches!(
                    proof_legacy_nodes.pop(),
                    Some(ProofTrieNode { path, .. }) if path.is_empty()
                );
                assert!(proof_legacy_nodes.is_empty());
            }

            // Basic comparison: both should succeed and produce identical results
            pretty_assertions::assert_eq!(proof_legacy_nodes, proof_v2_result);

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
            prop::collection::vec((any::<[u8; 32]>(), account_strategy()), 0..40).prop_map(
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
        /// and 20% random keys.
        fn proof_targets_strategy(account_keys: Vec<B256>) -> impl Strategy<Value = Vec<B256>> {
            let num_accounts = account_keys.len();

            // Generate between 0 and (num_accounts + 5) targets
            let target_count = 0..=(num_accounts + 5);

            target_count.prop_flat_map(move |count| {
                let account_keys = account_keys.clone();
                prop::collection::vec(
                    prop::bool::weighted(0.8).prop_flat_map(move |from_accounts| {
                        if from_accounts && !account_keys.is_empty() {
                            // 80% chance: pick from existing account keys
                            prop::sample::select(account_keys.clone()).boxed()
                        } else {
                            // 20% chance: generate random B256
                            any::<[u8; 32]>().prop_map(B256::from).boxed()
                        }
                    }),
                    count,
                )
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(8000))]

            /// Tests that ProofCalculator produces valid proofs for randomly generated
            /// HashedPostState with proof targets.
            ///
            /// This test:
            /// - Generates random accounts in a HashedPostState
            /// - Generates proof targets: 80% from existing account keys, 20% random
            /// - Creates a test harness with the generated state
            /// - Calls assert_proof with the generated targets
            /// - Verifies both ProofCalculator and legacy Proof produce equivalent results
            #[test]
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

                // Pass generated targets to both implementations
                harness.assert_proof(targets).expect("Proof generation failed");
            }
        }
    }
}
