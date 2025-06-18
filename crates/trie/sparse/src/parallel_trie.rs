use crate::{SparseNode, SparseTrieUpdates, TrieMasks};
use alloc::vec::Vec;
use alloy_primitives::{
    map::{Entry, HashMap},
    B256,
};
use alloy_rlp::Decodable;
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{Nibbles, TrieNode, CHILD_INDEX_RANGE};

/// A revealed sparse trie with subtries that can be updated in parallel.
///
/// ## Invariants
///
/// - Each leaf entry in the `subtries` and `upper_trie` collection must have a corresponding entry
///   in `values` collection. If the root node is a leaf, it must also have an entry in `values`.
/// - All keys in `values` collection are full leaf paths.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParallelSparseTrie {
    /// The root of the sparse trie.
    root_node: SparseNode,
    /// Map from a path (nibbles) to its corresponding sparse trie node.
    /// This contains the trie nodes for the upper part of the trie.
    upper_trie: HashMap<Nibbles, SparseNode>,
    /// An array containing the subtries at the second level of the trie.
    subtries: [Option<SparseSubtrie>; 256],
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            root_node: SparseNode::Empty,
            upper_trie: HashMap::default(),
            subtries: [const { None }; 256],
            values: HashMap::default(),
            updates: None,
        }
    }
}

// TODO maybe there's already a type somewhere which is basically this
enum TrieNodeOrHashToReveal {
    TrieNode(TrieNode),
    Hash(B256),
}

fn decode_node_or_hash(child: &[u8]) -> SparseTrieResult<TrieNodeOrHashToReveal> {
    if child.len() == B256::len_bytes() + 1 {
        let hash = B256::from_slice(&child[1..]);
        Ok(TrieNodeOrHashToReveal::Hash(hash))
    } else {
        Ok(TrieNodeOrHashToReveal::TrieNode(TrieNode::decode(&mut &child[..])?))
    }
}

/// RevealNodeOutcome gets returned from reveal_node, and can indicate further actions that the
/// calling subtrie needs to take.
enum RevealNodeOutcome {
    /// Continue indicates that there is nothing for the caller to do.
    Continue,
    /// LeavesRevealed indicates that one or more leaf nodes were revealed within whichever subtrie
    /// reveal_node was called on, and that their values therefore need to be added to the
    /// ParallelSparseTrie's values HashMap.
    LeavesRevealed(Vec<(Nibbles, Vec<u8>)>),
    /// RevealChildNode indicates that one or more child nodes of the revealed node can also be
    /// revealed.
    RevealChildNodes(Vec<(Nibbles, TrieNodeOrHashToReveal)>),
}

/// Reveals either a node or its hash placeholder.
///
/// When traversing the trie, we often encounter references to child nodes that
/// are either directly embedded or represented by their hash. This method
/// handles both cases:
///
/// 1. If the child data represents a hash (32+1=33 bytes), store it as a hash node
/// 2. Otherwise, decode the data as a [`TrieNode`] and recursively reveal it using `reveal_node`
///
/// # Returns
///
/// Returns `Ok(RevealNodeOutcome)` if successful, or an error if the node cannot be revealed.
///
/// # Error Handling
///
/// Will error if there's a conflict between a new hash node and an existing one
/// at the same path
fn reveal_node_or_hash(
    nodes: &mut HashMap<Nibbles, SparseNode>,
    path: Nibbles,
    node_or_hash: TrieNodeOrHashToReveal,
    masks: TrieMasks,
) -> SparseTrieResult<RevealNodeOutcome> {
    match node_or_hash {
        TrieNodeOrHashToReveal::Hash(hash) => {
            match nodes.entry(path) {
                Entry::Occupied(entry) => match entry.get() {
                    // Hash node with a different hash can't be handled.
                    SparseNode::Hash(previous_hash) if previous_hash != &hash => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: entry.key().clone(),
                            node: Box::new(SparseNode::Hash(hash)),
                        }
                        .into())
                    }
                    _ => {}
                },
                Entry::Vacant(entry) => {
                    entry.insert(SparseNode::Hash(hash));
                }
            }
            return Ok(RevealNodeOutcome::Continue)
        }
        TrieNodeOrHashToReveal::TrieNode(node) => reveal_node(nodes, path, node, masks),
    }
}

/// Reveals a trie node if it has not been revealed before.
///
/// This internal function decodes a trie node and inserts it into the nodes map. It handles
/// different node types (leaf, extension, branch) by appropriately adding them to the trie
/// structure. Children which can be recursively revealed are returned via the outcome enumeration.
///
/// # Returns
///
/// `Ok(())` if successful, or an error if node was not revealed.
fn reveal_node(
    nodes: &mut HashMap<Nibbles, SparseNode>,
    path: Nibbles,
    node: TrieNode,
    masks: TrieMasks,
) -> SparseTrieResult<RevealNodeOutcome> {
    // If the node is already revealed and it's not a hash node, do nothing.
    if nodes.get(&path).is_some_and(|node| !node.is_hash()) {
        return Ok(RevealNodeOutcome::Continue)
    }

    match node {
        TrieNode::EmptyRoot => {
            // For an empty root, ensure that we are at the root path.
            debug_assert!(path.is_empty());
            nodes.insert(path, SparseNode::Empty);
            Ok(RevealNodeOutcome::Continue)
        }
        TrieNode::Branch(branch) => {
            // For a branch node, iterate over all potential children
            let mut stack_ptr = branch.as_ref().first_child_index();
            let mut to_reveal = Vec::new();
            for idx in CHILD_INDEX_RANGE {
                if branch.state_mask.is_bit_set(idx) {
                    let mut child_path = path.clone();
                    child_path.push_unchecked(idx);
                    stack_ptr += 1;
                    to_reveal.push((child_path, decode_node_or_hash(&branch.stack[stack_ptr])?));
                }
            }

            // Update the branch node entry in the nodes map, handling cases where a blinded
            // node is now replaced with a revealed node.
            match nodes.entry(path) {
                Entry::Occupied(mut entry) => match entry.get() {
                    // Replace a hash node with a fully revealed branch node.
                    SparseNode::Hash(hash) => {
                        entry.insert(SparseNode::Branch {
                            state_mask: branch.state_mask,
                            // Memoize the hash of a previously blinded node in a new branch
                            // node.
                            hash: Some(*hash),
                            store_in_db_trie: Some(
                                masks.hash_mask.is_some_and(|mask| !mask.is_empty()) ||
                                    masks.tree_mask.is_some_and(|mask| !mask.is_empty()),
                            ),
                        });
                    }
                    // Branch node already exists, or an extension node was placed where a
                    // branch node was before.
                    SparseNode::Branch { .. } | SparseNode::Extension { .. } => {}
                    // All other node types can't be handled.
                    node @ (SparseNode::Empty | SparseNode::Leaf { .. }) => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: entry.key().clone(),
                            node: Box::new(node.clone()),
                        }
                        .into())
                    }
                },
                Entry::Vacant(entry) => {
                    entry.insert(SparseNode::new_branch(branch.state_mask));
                }
            }

            if to_reveal.len() > 0 {
                Ok(RevealNodeOutcome::RevealChildNodes(to_reveal))
            } else {
                Ok(RevealNodeOutcome::Continue)
            }
        }
        TrieNode::Extension(ext) => match nodes.entry(path) {
            Entry::Occupied(mut entry) => match entry.get() {
                // Replace a hash node with a revealed extension node.
                SparseNode::Hash(hash) => {
                    let mut child_path = entry.key().clone();
                    child_path.extend_from_slice_unchecked(&ext.key);
                    entry.insert(SparseNode::Extension {
                        key: ext.key,
                        // Memoize the hash of a previously blinded node in a new extension
                        // node.
                        hash: Some(*hash),
                        store_in_db_trie: None,
                    });
                    Ok(RevealNodeOutcome::RevealChildNodes(vec![(
                        child_path,
                        decode_node_or_hash(&ext.child)?,
                    )]))
                }
                // Extension node already exists, or an extension node was placed where a branch
                // node was before.
                SparseNode::Extension { .. } | SparseNode::Branch { .. } => {
                    Ok(RevealNodeOutcome::Continue)
                }
                // All other node types can't be handled.
                node @ (SparseNode::Empty | SparseNode::Leaf { .. }) => {
                    Err(SparseTrieErrorKind::Reveal {
                        path: entry.key().clone(),
                        node: Box::new(node.clone()),
                    }
                    .into())
                }
            },
            Entry::Vacant(entry) => {
                let mut child_path = entry.key().clone();
                child_path.extend_from_slice_unchecked(&ext.key);
                entry.insert(SparseNode::new_ext(ext.key));
                Ok(RevealNodeOutcome::RevealChildNodes(vec![(
                    child_path,
                    decode_node_or_hash(&ext.child)?,
                )]))
            }
        },
        TrieNode::Leaf(leaf) => match nodes.entry(path) {
            Entry::Occupied(mut entry) => match entry.get() {
                // Replace a hash node with a revealed leaf node and store leaf node value.
                SparseNode::Hash(hash) => {
                    let mut full = entry.key().clone();
                    full.extend_from_slice_unchecked(&leaf.key);
                    entry.insert(SparseNode::Leaf {
                        key: leaf.key,
                        // Memoize the hash of a previously blinded node in a new leaf
                        // node.
                        hash: Some(*hash),
                    });

                    Ok(RevealNodeOutcome::LeavesRevealed(vec![(full, leaf.value)]))
                }
                // Leaf node already exists.
                SparseNode::Leaf { .. } => Ok(RevealNodeOutcome::Continue),
                // All other node types can't be handled.
                node @ (SparseNode::Empty |
                SparseNode::Extension { .. } |
                SparseNode::Branch { .. }) => Err(SparseTrieErrorKind::Reveal {
                    path: entry.key().clone(),
                    node: Box::new(node.clone()),
                }
                .into()),
            },
            Entry::Vacant(entry) => {
                let mut full = entry.key().clone();
                full.extend_from_slice_unchecked(&leaf.key);
                entry.insert(SparseNode::new_leaf(leaf.key));
                Ok(RevealNodeOutcome::LeavesRevealed(vec![(full, leaf.value)]))
            }
        },
    }
}

impl ParallelSparseTrie {
    fn subtrie_for_path(&mut self, path: &Nibbles) -> Option<&mut SparseSubtrie> {
        // get the index into subtries array using upper two nibbles of the path
        let idx =
            path.split_at_checked(2).map(|(upper, _)| ((upper[0] << 4) | upper[1]) as usize)?;
        let upper_path = Nibbles::unpack([idx as u8]);

        if self.subtries[idx].is_none() {
            self.subtries[idx] = Some(SparseSubtrie::new(upper_path));
        }
        self.subtries[idx].as_mut()
    }

    fn reveal_node_or_hash(
        &mut self,
        path: Nibbles,
        node_or_hash: TrieNodeOrHashToReveal,
        masks: TrieMasks,
    ) -> SparseTrieResult<()> {
        // TODO parallelize
        if let Some(subtrie) = self.subtrie_for_path(&path) {
            if let Some(leaves) = subtrie.reveal_node_or_hash(path, node_or_hash, masks)? {
                for (path, value) in leaves {
                    self.values.insert(path, value);
                }
            }

            return Ok(())
        }

        // If there is no subtrie for the path it means the path is 2 or less nibbles, and so
        // belongs to the upper trie. We reveal the node in the upper trie, but if further child
        // nodes need to be revealed we have to recurse in case they belong to a subtrie.
        match reveal_node_or_hash(&mut self.upper_trie, path, node_or_hash, masks)? {
            RevealNodeOutcome::Continue => (),
            RevealNodeOutcome::LeavesRevealed(leaves) => {
                for (path, value) in leaves {
                    self.values.insert(path, value);
                }
            }
            RevealNodeOutcome::RevealChildNodes(children) => {
                for (path, node_or_hash) in children {
                    self.reveal_node_or_hash(path, node_or_hash, TrieMasks::none())?;
                }
            }
        }

        Ok(())
    }

    /// Reveals a trie node if it has not been revealed before.
    ///
    /// This internal function decodes a trie node and inserts it into the nodes map.
    /// It handles different node types (leaf, extension, branch) by appropriately
    /// adding them to the trie structure and recursively revealing their children.
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if node was not revealed.
    pub fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNode,
        masks: TrieMasks,
    ) -> SparseTrieResult<()> {
        self.reveal_node_or_hash(path, TrieNodeOrHashToReveal::TrieNode(node), masks)
    }
}

/// This is a subtrie of the `ParallelSparseTrie` that contains a map from path to sparse trie
/// nodes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SparseSubtrie {
    /// The root path of this subtrie.
    path: Nibbles,
    /// The map from paths to sparse trie nodes within this subtrie.
    nodes: HashMap<Nibbles, SparseNode>,
}

impl SparseSubtrie {
    fn new(path: Nibbles) -> Self {
        Self { path, nodes: Default::default() }
    }

    /// Internal implementation of the method of the same name from ParallelSparseTrie. This method
    /// may return leaf paths/values which need to be added to the ParallelSparseTrie's values
    /// HashMap.
    fn reveal_node_or_hash(
        &mut self,
        path: Nibbles,
        node_or_hash: TrieNodeOrHashToReveal,
        masks: TrieMasks,
    ) -> SparseTrieResult<Option<Vec<(Nibbles, Vec<u8>)>>> {
        match reveal_node_or_hash(&mut self.nodes, path, node_or_hash, masks)? {
            RevealNodeOutcome::Continue => Ok(None),
            RevealNodeOutcome::LeavesRevealed(leaves) => Ok(Some(leaves)),
            RevealNodeOutcome::RevealChildNodes(children) => {
                let mut leaves_revealed = Vec::new();
                for (path, child) in children {
                    if let Some(leaves) =
                        self.reveal_node_or_hash(path.clone(), child, TrieMasks::none())?
                    {
                        leaves_revealed.extend(leaves);
                    }
                }

                if leaves_revealed.len() > 0 {
                    Ok(Some(leaves_revealed))
                } else {
                    Ok(None)
                }
            }
        }
    }
}
