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
use reth_trie_common::{Nibbles, RlpNode, SparseTrieNode, TrieMasks, TrieNode};

mod targets;
pub use targets::*;

mod value;
pub use value::*;

mod node;
use node::*;

/// A proof calculator that generates merkle proofs using only leaf data.
///
/// The calculator:
/// - Accepts one or more B256 proof targets sorted lexicographically
/// - Returns proof nodes sorted lexicographically by path
/// - Returns only the root when given zero targets
/// - Automatically resets after each calculation
/// - Re-uses cursors from one calculation to the next
#[derive(Debug)]
pub struct ProofCalculator<TC, HC, VE: ValueEncoder> {
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
    child_stack: Vec<ProofTrieBranchChild<VE::Fut>>,
    /// Re-usable buffer of [`RlpNode`]s, used for encoding branch nodes to RLP.
    rlp_node_buf: Vec<RlpNode>,
    /// Re-usable byte buffer, used for RLP encoding.
    rlp_encode_buf: Vec<u8>,
}

impl<TC, HC, VE: ValueEncoder> ProofCalculator<TC, HC, VE> {
    /// Create a new [`ProofCalculator`] instance for calculating account proofs.
    pub const fn new(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self {
            trie_cursor,
            hashed_cursor,
            branch_stack: Vec::<_>::new(),
            branch_path: Nibbles::new(),
            child_stack: Vec::<_>::new(),
            rlp_node_buf: Vec::<_>::new(),
            rlp_encode_buf: Vec::<_>::new(),
        }
    }
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: ValueEncoder<Value = HC::Value>,
{
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
        let branch = self.branch_stack.pop().expect("branch_stack cannot be empty");

        // Take the branch's children off the stack, using the state mask to determine how many
        // there are.
        let num_children = branch.state_mask.count_ones() as usize;
        debug_assert!(num_children > 1, "A branch must have at least two children");
        debug_assert!(
            self.child_stack.len() >= num_children,
            "Stack is missing necessary children"
        );
        let children = self.child_stack.drain(self.child_stack.len() - num_children..);

        // Update the branch_path, removing the nibble for this branch and any nibble's from its
        // parent extension as well (if there is one).
        debug_assert!(self.branch_path.len() > 1 + branch.ext_len as usize);
        let new_path_len = self.branch_path.len() - 1 - branch.ext_len as usize;
        self.branch_path = self.branch_path.slice_unchecked(0, new_path_len);

        // Encode the branch node and push it onto the child stack, replacing its children.
        let branch_rlp_node = branch.into_rlp(
            &mut self.rlp_encode_buf,
            &mut self.rlp_node_buf,
            children,
            &self.branch_path,
        )?;
        self.child_stack.push(ProofTrieBranchChild::RlpNode(branch_rlp_node));

        Ok(())
    }

    /// Adds a single leaf for a key to the stack, possibly collapsing an existing branch and/or
    /// creating a new one depending on the path of the key.
    fn add_leaf(&mut self, key: Nibbles, val: VE::Fut) -> Result<(), StateProofError> {
        loop {
            // Get the branch currently being built. If there are no branches on the stack then it
            // means either the trie is empty or only a single leaf has been added previously.
            let curr_branch = match self.branch_stack.last_mut() {
                Some(curr_branch) => curr_branch,
                None if self.child_stack.is_empty() => {
                    // if the child stack is empty then this is the first leaf, push it and be done
                    self.child_stack
                        .push(ProofTrieBranchChild::Leaf { short_key: key, value: val });
                    return Ok(());
                }
                None => todo!("assert existing child is a leaf, create branch"),
            };

            // Find the common prefix length, which is the number of nibbles shared between the
            // current branch and the key.
            let prefix_len = self.branch_path.common_prefix_length(&key);
            debug_assert!(prefix_len <= self.branch_path.len());

            // If current branch is a prefix of the new key then the leaf is a child of the branch
            // and can be added directly.
            if prefix_len == self.branch_path.len() {
                // Set the bit for the leaf's nibble into the branch's state mask.
                let nibble = key.get_unchecked(prefix_len);
                debug_assert!(
                    !curr_branch.state_mask.is_bit_set(nibble),
                    "Nibble already set in state mask"
                );
                curr_branch.state_mask.set_bit(nibble);

                // Push the leaf onto the stack.
                self.child_stack.push(ProofTrieBranchChild::Leaf {
                    short_key: key.slice_unchecked(prefix_len + 1, key.len()),
                    value: val,
                });
                return Ok(());
            }

            // If the current branch and the new key only share a prefix then the current branch
            // will have no more children. We can commit it and loop back to the top to
            // try again with the parent branch.
            self.pop_branch()?;
        }
    }

    /// Internal implementation of proof calculation. Assumes both cursors have already been reset.
    /// See docs on [`Self::proof`] for expected behavior.
    fn proof_inner(
        &mut self,
        value_encoder: &VE,
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

        loop {
            // Fetch the next leaft from the hashed cursor, converting the key to Nibbles and
            // immediately creating the ValueEncoderFut so that encoding of the leaf value can begin
            // ASAP.
            let Some((key, val)) = self.hashed_cursor.next()?.map(|(key, val)| {
                debug_assert_eq!(key.len(), 32);
                // SAFETY: key is a B256 and so is exactly 32-bytes.
                let key = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
                let val = value_encoder.encoder_fut(val);
                (key, val)
            }) else {
                todo!("collapse remaining stack")
            };

            self.add_leaf(key, val)?;
        }
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
