use crate::{
    Nibbles, TrieMask,
    proof::{ProofNodes, added_removed_keys::AddedRemovedKeys},
};
use alloy_primitives::Bytes;
use alloy_rlp::EMPTY_STRING_CODE;
use tracing::trace;

use alloc::vec::Vec;

/// Tracks various datapoints related to already-seen proofs which may or may not have been
/// retained in the outer [`ProofRetainer`]. Used to support retention of extra proofs which are
/// required to support leaf additions/removals but aren't in the target set.
///
/// "target" refers those paths/proofs which are retained by the [`ProofRetainer`], as determined
/// by its `target` field.
#[derive(Default, Clone, Debug)]
struct AddedRemovedKeysTracking {
    /// Path of the last branch which was passed to `retain_branch_proof`, regardless of whether it
    /// was a target or not.
    last_branch_path: Nibbles,
    /// Proof of the node at `last_branch_path`.
    last_branch_proof: Vec<u8>,
    /// Path of the last node which was not found in the `ProofRetainer`'s target set, and which is
    /// not potentially removed as given by [`AddedRemovedKeys::get_removed_mask`]
    last_nontarget_path: Nibbles,
    /// Proof of the node at `last_nontarget_path`.
    last_nontarget_proof: Vec<u8>,
}

impl AddedRemovedKeysTracking {
    /// Stores the path and proof of the most recently seen path/proof which were not retained and
    /// are not potentially removed.
    fn track_nontarget(&mut self, path: &Nibbles, proof: &[u8]) {
        trace!(target: "trie::proof_retainer", ?path, "Tracking non-target");
        self.last_nontarget_path = *path;
        self.last_nontarget_proof.clear();
        self.last_nontarget_proof.extend(proof);
    }

    /// Stores the path and proof of the most recently seen branch.
    fn track_branch(&mut self, path: &Nibbles, proof: &[u8]) {
        trace!(target: "trie::proof_retainer", ?path, "Tracking branch");
        self.last_branch_path = *path;
        self.last_branch_proof.clear();
        self.last_branch_proof.extend(proof);
    }

    /// Checks if an extension node is parent to the last tracked branch node.
    fn extension_child_is_nontarget(&self, path: &Nibbles, short_key: &Nibbles) -> bool {
        path.len() + short_key.len() == self.last_nontarget_path.len()
            && self.last_nontarget_path.starts_with(path)
            && self.last_nontarget_path.ends_with(short_key)
    }

    /// Checks if a branch node is parent to the last non-target node.
    fn branch_child_is_nontarget(&self, path: &Nibbles, child_nibble: u8) -> bool {
        path.len() + 1 == self.last_nontarget_path.len()
            && self.last_nontarget_path.starts_with(path)
            && self.last_nontarget_path.last().expect("path length >= 1") == child_nibble
    }
}

/// Proof retainer is used to store proofs during merkle trie construction.
/// It is intended to be used within the [`HashBuilder`](crate::HashBuilder).
///
/// When using the `retain_leaf_proof`, `retain_extension_proof`, and `retain_branch_proof`
/// methods, it is required that the calls are ordered such that proofs of parent nodes are
/// retained after their children.
#[derive(Default, Clone, Debug)]
pub struct ProofRetainer<K = AddedRemovedKeys> {
    /// The nibbles of the target trie keys to retain proofs for.
    targets: Vec<Nibbles>,
    /// The map retained trie node keys to RLP serialized trie nodes.
    proof_nodes: ProofNodes,
    /// Provided by the user to give the necessary context to retain extra proofs.
    added_removed_keys: Option<K>,
    /// Tracks data related to previously seen proofs; required for certain edge-cases where we
    /// want to keep proofs for nodes which aren't in the target set.
    added_removed_tracking: AddedRemovedKeysTracking,
}

impl FromIterator<Nibbles> for ProofRetainer {
    fn from_iter<T: IntoIterator<Item = Nibbles>>(iter: T) -> Self {
        Self::new(FromIterator::from_iter(iter))
    }
}

impl ProofRetainer {
    /// Create new retainer with target nibbles.
    pub fn new(targets: Vec<Nibbles>) -> Self {
        Self {
            targets,
            ..Default::default()
        }
    }
}

impl<K> ProofRetainer<K> {
    /// Configures the retainer to retain proofs for certain nodes which would otherwise fall
    /// outside the target set, when those nodes might be required to calculate the state root when
    /// keys have been added or removed to the trie.
    ///
    /// If None is given then retention of extra proofs is disabled.
    pub fn with_added_removed_keys<K2>(self, added_removed_keys: Option<K2>) -> ProofRetainer<K2> {
        ProofRetainer {
            targets: self.targets,
            proof_nodes: self.proof_nodes,
            added_removed_keys,
            added_removed_tracking: self.added_removed_tracking,
        }
    }
}

impl<K: AsRef<AddedRemovedKeys>> ProofRetainer<K> {
    /// Returns `true` if the given prefix matches the retainer target.
    pub fn matches(&self, prefix: &Nibbles) -> bool {
        prefix.is_empty() || self.targets.iter().any(|target| target.starts_with(prefix))
    }

    /// Returns all collected proofs.
    pub fn into_proof_nodes(self) -> ProofNodes {
        self.proof_nodes
    }

    /// Retain the proof if the key matches any of the targets.
    ///
    /// Usage of this method should be replaced with usage of the following methods, each dependent
    /// on the node-type whose proof being retained:
    /// - `retain_empty_root_proof`
    /// - `retain_leaf_proof`
    /// - `retain_extension_proof`
    /// - `retain_branch_proof`
    #[deprecated]
    pub fn retain(&mut self, prefix: &Nibbles, proof: &[u8]) {
        if self.matches(prefix) {
            self.retain_unchecked(*prefix, Bytes::copy_from_slice(proof));
        }
    }

    /// Retain the proof with no checks being performed.
    fn retain_unchecked(&mut self, path: Nibbles, proof: Bytes) {
        trace!(
            target: "trie::proof_retainer",
            path = ?path,
            proof = alloy_primitives::hex::encode(&proof),
            "Retaining proof",
        );
        self.proof_nodes.insert(path, proof);
    }

    /// Retains a proof for an empty root.
    pub fn retain_empty_root_proof(&mut self) {
        self.retain_unchecked(Nibbles::default(), Bytes::from_static(&[EMPTY_STRING_CODE]))
    }

    /// Tracks the proof in the [`AddedRemovedKeysTracking`] if:
    /// - Tracking is enabled
    /// - Path is not root
    /// - The path is not a removed child as given by [`AddedRemovedKeys`]
    ///
    /// Non-target tracking is only used for retaining of extra branch children in cases where the
    /// branch is getting deleted, hence why root node is not kept.
    fn maybe_track_nontarget(&mut self, path: &Nibbles, proof: &[u8]) {
        if let Some(added_removed_keys) = self.added_removed_keys.as_ref() {
            if path.is_empty() {
                return;
            }

            let branch_path = path.slice_unchecked(0, path.len() - 1);
            let child_bit = path.get_unchecked(path.len() - 1);
            let removed_mask = added_removed_keys.as_ref().get_removed_mask(&branch_path);
            if !removed_mask.is_bit_set(child_bit) {
                self.added_removed_tracking.track_nontarget(path, proof)
            }
        }
    }

    /// Retains a proof for a leaf node.
    pub fn retain_leaf_proof(&mut self, path: &Nibbles, proof: &[u8]) {
        if self.matches(path) {
            self.retain_unchecked(*path, Bytes::copy_from_slice(proof));
        } else {
            self.maybe_track_nontarget(path, proof);
        }
    }

    /// Retains a proof for an extension node.
    pub fn retain_extension_proof(&mut self, path: &Nibbles, short_key: &Nibbles, proof: &[u8]) {
        if self.matches(path) {
            self.retain_unchecked(*path, Bytes::copy_from_slice(proof));

            if let Some(added_removed_keys) = self.added_removed_keys.as_ref() {
                // When a new leaf is being added to a trie, it can happen that an extension node's
                // path is a prefix of the new leaf's, but the extension child's path is not. In
                // this case a new branch node is created; the new leaf is one child, the previous
                // branch node is its other, and the extension node is its parent.
                //
                //            Before │ After
                //                   │
                //   ┌───────────┐   │    ┌───────────┐
                //   │ Extension │   │    │ Extension │
                //   └─────┬─────┘   │    └─────┬─────┘
                //         │         │          │
                //         │         │    ┌─────┴──────┐
                //         │         │    │ New Branch │
                //         │         │    └─────┬───┬──┘
                //         │         │          │   └─────┐
                //   ┌─────┴────┐    │    ┌─────┴────┐  ┌─┴────────┐
                //   │  Branch  │    │    │  Branch  │  │ New Leaf │
                //   └──────────┘    │    └──────────┘  └──────────┘
                //
                // In this case the new leaf's proof will be retained, as will the extension's,
                // because its path is a prefix of the leaf's. But the old branch's proof won't
                // necessarily be retained, as its path is not a prefix of the leaf's.
                //
                // In order to support this case we can optimistically retain the proof for
                // non-target children of target extensions.
                //
                let is_prefix_added = added_removed_keys.as_ref().is_prefix_added(path);
                let extension_child_is_nontarget = self
                    .added_removed_tracking
                    .extension_child_is_nontarget(path, short_key);
                trace!(
                    target: "trie::proof_retainer",
                    ?path,
                    ?short_key,
                    ?is_prefix_added,
                    ?extension_child_is_nontarget,
                    "Deciding to retain non-target extension child",
                );
                if is_prefix_added && extension_child_is_nontarget {
                    let last_branch_path = self.added_removed_tracking.last_branch_path;
                    let last_branch_proof =
                        Bytes::copy_from_slice(&self.added_removed_tracking.last_branch_proof);
                    self.retain_unchecked(last_branch_path, last_branch_proof);
                }
            }
        } else {
            self.maybe_track_nontarget(path, proof);
        }
    }

    /// Retains a proof for a branch node.
    pub fn retain_branch_proof(&mut self, path: &Nibbles, state_mask: TrieMask, proof: &[u8]) {
        if self.matches(path) {
            self.retain_unchecked(*path, Bytes::copy_from_slice(proof));

            if let Some(added_removed_keys) = self.added_removed_keys.as_ref() {
                // When we remove all but one child from a branch, that branch gets "collapsed"
                // into its parent branch/extension, i.e. it is deleted and the remaining child is
                // adopted by its grandparent.
                //
                //                           Before │ After
                //                                  │
                //   ┌───────────────┐              │    ┌───────────────┐
                //   │  Grandparent  │              │    │  Grandparent  │
                //   │  Branch       │              │    │  Branch       │
                //   └──┬───┬───┬────┘              │    └──┬───┬───┬────┘
                //      :   :   │                   │       :   :   │
                //              │                   │               │
                //   ┌──────────┴──────┐            │               │
                //   │  Parent Branch  │            │               │
                //   └──────────┬──┬───┘            │               │
                //              │  └─────┐          │               │
                //   ┌──────────┴─────┐ ┌┴─────┐    │    ┌──────────┴─────┐
                //   │  Child Branch  │ │ Leaf │    │    │  Child Branch  │
                //   └──┬───┬───┬─────┘ └──────┘    │    └──┬───┬───┬─────┘
                //      :   :   :                   │       :   :   :
                //
                // The adopted child can also be a leaf or extension, which can affect what happens
                // to the grandparent, so to perform a collapse we must retain a proof of the
                // adopted child.
                //
                // The proofs of the removed children will be retained if they are in the `target`
                // set, but it can happen that the remaining child will not be in the `target` set
                // and so would not be retained.
                //
                // Using `removed_keys` we can discern if there is one remaining child in a branch
                // which is not a target, and optimistically retain that child if so.
                //
                let removed_mask = added_removed_keys.as_ref().get_removed_mask(path);
                let nonremoved_mask = !removed_mask & state_mask;
                let branch_child_is_nontarget = self
                    .added_removed_tracking
                    .branch_child_is_nontarget(path, nonremoved_mask.trailing_zeros() as u8);

                trace!(
                    target: "trie::proof_retainer",
                    ?path,
                    ?removed_mask,
                    ?state_mask,
                    ?nonremoved_mask,
                    ?branch_child_is_nontarget,
                    "Deciding to retain non-target branch child",
                );

                if nonremoved_mask.count_ones() == 1 && branch_child_is_nontarget {
                    let last_nontarget_path = self.added_removed_tracking.last_nontarget_path;
                    let last_nontarget_proof =
                        Bytes::copy_from_slice(&self.added_removed_tracking.last_nontarget_proof);
                    self.retain_unchecked(last_nontarget_path, last_nontarget_proof);
                }
            }
        } else {
            self.maybe_track_nontarget(path, proof);
        }

        if self.added_removed_keys.is_some() {
            self.added_removed_tracking.track_branch(path, proof)
        }
    }
}
