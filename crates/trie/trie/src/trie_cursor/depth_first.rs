use super::TrieCursor;
use crate::{BranchNodeCompact, Nibbles};
use reth_storage_errors::db::DatabaseError;
use std::cmp::Ordering;
use tracing::trace;

/// Compares two Nibbles in depth-first order.
///
/// In depth-first ordering:
/// - Descendants come before their ancestors (children before parents)
/// - Siblings are ordered lexicographically
///
/// # Example
///
/// ```text
/// 0x11 comes before 0x1 (child before parent)
/// 0x12 comes before 0x1 (child before parent)
/// 0x11 comes before 0x12 (lexicographical among siblings)
/// 0x1 comes before 0x21 (lexicographical among siblings)
/// Result: 0x11, 0x12, 0x1, 0x21
/// ```
pub fn cmp(a: &Nibbles, b: &Nibbles) -> Ordering {
    // If the two are equal length then compare them lexicographically
    if a.len() == b.len() {
        return a.cmp(b)
    }

    // If one is a prefix of the other, then the other comes first
    let common_prefix_len = a.common_prefix_length(b);
    if a.len() == common_prefix_len {
        return Ordering::Greater
    } else if b.len() == common_prefix_len {
        return Ordering::Less
    }

    // Otherwise the nibble after the prefix determines the ordering. We know that neither is empty
    // at this point, otherwise the previous if/else block would have caught it.
    a.get_unchecked(common_prefix_len).cmp(&b.get_unchecked(common_prefix_len))
}

/// An iterator that traverses trie nodes in depth-first post-order.
///
/// This iterator yields nodes in post-order traversal (children before parents),
/// which matches the `cmp` comparison function where descendants
/// come before their ancestors.
#[derive(Debug)]
pub struct DepthFirstTrieIterator<C: TrieCursor> {
    /// The underlying trie cursor.
    cursor: C,
    /// Set to true once the trie cursor has done its initial seek to the root node.
    initialized: bool,
    /// Stack of nodes which have been fetched. Each node's path is a prefix of the next's.
    stack: Vec<(Nibbles, BranchNodeCompact)>,
    /// Nodes which are ready to be yielded from `next`.
    next: Vec<(Nibbles, BranchNodeCompact)>,
    /// Set to true once the cursor has been exhausted.
    complete: bool,
}

impl<C: TrieCursor> DepthFirstTrieIterator<C> {
    /// Create a new depth-first iterator from a trie cursor.
    pub fn new(cursor: C) -> Self {
        Self {
            cursor,
            initialized: false,
            stack: Default::default(),
            next: Default::default(),
            complete: false,
        }
    }

    fn push(&mut self, path: Nibbles, node: BranchNodeCompact) {
        loop {
            match self.stack.last() {
                None => {
                    // If the stack is empty then we push this node onto it, as it may have child
                    // nodes which need to be yielded first.
                    self.stack.push((path, node));
                    break
                }
                Some((top_path, _)) if path.starts_with(top_path) => {
                    // If the top of the stack is a prefix of this node, it means this node is a
                    // child of the top of the stack (and all other nodes on the stack). Push this
                    // node onto the stack, as future nodes may be children of it.
                    self.stack.push((path, node));
                    break
                }
                Some((_, _)) => {
                    // The top of the stack is not a prefix of this node, therefore it is not a
                    // parent of this node. Yield the top of the stack, and loop back to see if this
                    // node is a child of the new top-of-stack.
                    self.next.push(self.stack.pop().expect("stack is not empty"));
                }
            }
        }

        // We will have popped off the top of the stack in the order we want to yield nodes, but
        // `next` is itself popped off so it needs to be reversed.
        self.next.reverse();
    }

    fn fill_next(&mut self) -> Result<(), DatabaseError> {
        debug_assert!(self.next.is_empty());

        loop {
            let Some((path, node)) = (if self.initialized {
                self.cursor.next()?
            } else {
                self.initialized = true;
                self.cursor.seek(Nibbles::new())?
            }) else {
                // Record that the cursor is empty and yield the stack. The stack is in reverse
                // order of what we want to yield, but `next` is popped from, so we don't have to
                // reverse it.
                self.complete = true;
                self.next = core::mem::take(&mut self.stack);
                return Ok(())
            };

            trace!(
                target: "trie::trie_cursor::depth_first",
                ?path,
                "Iterated from cursor",
            );

            self.push(path, node);
            if !self.next.is_empty() {
                return Ok(())
            }
        }
    }
}

impl<C: TrieCursor> Iterator for DepthFirstTrieIterator<C> {
    type Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(next) = self.next.pop() {
                return Some(Ok(next))
            }

            if self.complete {
                return None
            }

            if let Err(err) = self.fill_next() {
                return Some(Err(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory};
    use alloy_trie::TrieMask;
    use std::{collections::BTreeMap, sync::Arc};

    fn create_test_node(state_nibbles: &[u8], tree_nibbles: &[u8]) -> BranchNodeCompact {
        let mut state_mask = TrieMask::default();
        for &nibble in state_nibbles {
            state_mask.set_bit(nibble);
        }

        let mut tree_mask = TrieMask::default();
        for &nibble in tree_nibbles {
            tree_mask.set_bit(nibble);
        }

        BranchNodeCompact {
            state_mask,
            tree_mask,
            hash_mask: TrieMask::default(),
            hashes: Arc::new(vec![]),
            root_hash: None,
        }
    }

    #[test]
    fn test_depth_first_cmp() {
        // Test case 1: Child comes before parent
        let child = Nibbles::from_nibbles([0x1, 0x1]);
        let parent = Nibbles::from_nibbles([0x1]);
        assert_eq!(cmp(&child, &parent), Ordering::Less);
        assert_eq!(cmp(&parent, &child), Ordering::Greater);

        // Test case 2: Deeper descendant comes before ancestor
        let deep = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
        let ancestor = Nibbles::from_nibbles([0x1, 0x2]);
        assert_eq!(cmp(&deep, &ancestor), Ordering::Less);
        assert_eq!(cmp(&ancestor, &deep), Ordering::Greater);

        // Test case 3: Siblings use lexicographical ordering
        let sibling1 = Nibbles::from_nibbles([0x1, 0x2]);
        let sibling2 = Nibbles::from_nibbles([0x1, 0x3]);
        assert_eq!(cmp(&sibling1, &sibling2), Ordering::Less);
        assert_eq!(cmp(&sibling2, &sibling1), Ordering::Greater);

        // Test case 4: Different branches use lexicographical ordering
        let branch1 = Nibbles::from_nibbles([0x1]);
        let branch2 = Nibbles::from_nibbles([0x2]);
        assert_eq!(cmp(&branch1, &branch2), Ordering::Less);
        assert_eq!(cmp(&branch2, &branch1), Ordering::Greater);

        // Test case 5: Empty path comes after everything
        let empty = Nibbles::new();
        let non_empty = Nibbles::from_nibbles([0x0]);
        assert_eq!(cmp(&non_empty, &empty), Ordering::Less);
        assert_eq!(cmp(&empty, &non_empty), Ordering::Greater);

        // Test case 6: Same paths are equal
        let same1 = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let same2 = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        assert_eq!(cmp(&same1, &same2), Ordering::Equal);
    }

    #[test]
    fn test_depth_first_ordering_complex() {
        // Test the example from the conversation: 0x11, 0x12, 0x1, 0x2
        let mut paths = [
            Nibbles::from_nibbles([0x1]),      // 0x1
            Nibbles::from_nibbles([0x2]),      // 0x2
            Nibbles::from_nibbles([0x1, 0x1]), // 0x11
            Nibbles::from_nibbles([0x1, 0x2]), // 0x12
        ];

        // Shuffle to ensure sorting works regardless of input order
        paths.reverse();

        // Sort using depth-first ordering
        paths.sort_by(cmp);

        // Expected order: 0x11, 0x12, 0x1, 0x2
        assert_eq!(paths[0], Nibbles::from_nibbles([0x1, 0x1])); // 0x11
        assert_eq!(paths[1], Nibbles::from_nibbles([0x1, 0x2])); // 0x12
        assert_eq!(paths[2], Nibbles::from_nibbles([0x1])); // 0x1
        assert_eq!(paths[3], Nibbles::from_nibbles([0x2])); // 0x2
    }

    #[test]
    fn test_depth_first_ordering_tree() {
        // Test a more complex tree structure
        let mut paths = vec![
            Nibbles::new(),                         // root (empty)
            Nibbles::from_nibbles([0x1]),           // 0x1
            Nibbles::from_nibbles([0x1, 0x1]),      // 0x11
            Nibbles::from_nibbles([0x1, 0x1, 0x1]), // 0x111
            Nibbles::from_nibbles([0x1, 0x1, 0x2]), // 0x112
            Nibbles::from_nibbles([0x1, 0x2]),      // 0x12
            Nibbles::from_nibbles([0x2]),           // 0x2
            Nibbles::from_nibbles([0x2, 0x1]),      // 0x21
        ];

        // Shuffle
        paths.reverse();

        // Sort using depth-first ordering
        paths.sort_by(cmp);

        // Expected depth-first order:
        // All descendants come before ancestors
        // Within same level, lexicographical order
        assert_eq!(paths[0], Nibbles::from_nibbles([0x1, 0x1, 0x1])); // 0x111 (deepest in 0x1 branch)
        assert_eq!(paths[1], Nibbles::from_nibbles([0x1, 0x1, 0x2])); // 0x112 (sibling of 0x111)
        assert_eq!(paths[2], Nibbles::from_nibbles([0x1, 0x1])); // 0x11 (parent of 0x111, 0x112)
        assert_eq!(paths[3], Nibbles::from_nibbles([0x1, 0x2])); // 0x12 (sibling of 0x11)
        assert_eq!(paths[4], Nibbles::from_nibbles([0x1])); // 0x1 (parent of 0x11, 0x12)
        assert_eq!(paths[5], Nibbles::from_nibbles([0x2, 0x1])); // 0x21 (child of 0x2)
        assert_eq!(paths[6], Nibbles::from_nibbles([0x2])); // 0x2 (parent of 0x21)
        assert_eq!(paths[7], Nibbles::new()); // root (empty, parent of all)
    }

    #[test]
    fn test_empty_trie() {
        let factory = MockTrieCursorFactory::new(BTreeMap::new(), Default::default());
        let cursor = factory.account_trie_cursor().unwrap();
        let mut iter = DepthFirstTrieIterator::new(cursor);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_single_node() {
        let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let node = create_test_node(&[0x4], &[0x5]);

        let mut nodes = BTreeMap::new();
        nodes.insert(path, node.clone());
        let factory = MockTrieCursorFactory::new(nodes, Default::default());
        let cursor = factory.account_trie_cursor().unwrap();
        let mut iter = DepthFirstTrieIterator::new(cursor);

        let result = iter.next().unwrap().unwrap();
        assert_eq!(result.0, path);
        assert_eq!(result.1, node);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_depth_first_order() {
        // Create a simple trie structure:
        // root
        // ├── 0x1 (has children 0x2 and 0x3)
        // │   ├── 0x12
        // │   └── 0x13
        // └── 0x2 (has child 0x4)
        //     └── 0x24

        let nodes = vec![
            // Root node with children at nibbles 1 and 2
            (Nibbles::default(), create_test_node(&[], &[0x1, 0x2])),
            // Node at path 0x1 with children at nibbles 2 and 3
            (Nibbles::from_nibbles([0x1]), create_test_node(&[], &[0x2, 0x3])),
            // Leaf nodes
            (Nibbles::from_nibbles([0x1, 0x2]), create_test_node(&[0xF], &[])),
            (Nibbles::from_nibbles([0x1, 0x3]), create_test_node(&[0xF], &[])),
            // Node at path 0x2 with child at nibble 4
            (Nibbles::from_nibbles([0x2]), create_test_node(&[], &[0x4])),
            // Leaf node
            (Nibbles::from_nibbles([0x2, 0x4]), create_test_node(&[0xF], &[])),
        ];

        let nodes_map: BTreeMap<_, _> = nodes.into_iter().collect();
        let factory = MockTrieCursorFactory::new(nodes_map, Default::default());
        let cursor = factory.account_trie_cursor().unwrap();
        let iter = DepthFirstTrieIterator::new(cursor);

        // Expected post-order (depth-first with children before parents):
        // 1. 0x12 (leaf, child of 0x1)
        // 2. 0x13 (leaf, child of 0x1)
        // 3. 0x1 (parent of 0x12 and 0x13)
        // 4. 0x24 (leaf, child of 0x2)
        // 5. 0x2 (parent of 0x24)
        // 6. Root (parent of 0x1 and 0x2)

        let expected_order = vec![
            Nibbles::from_nibbles([0x1, 0x2]),
            Nibbles::from_nibbles([0x1, 0x3]),
            Nibbles::from_nibbles([0x1]),
            Nibbles::from_nibbles([0x2, 0x4]),
            Nibbles::from_nibbles([0x2]),
            Nibbles::default(),
        ];

        let mut actual_order = Vec::new();
        for result in iter {
            let (path, _) = result.unwrap();
            actual_order.push(path);
        }

        assert_eq!(actual_order, expected_order);
    }

    #[test]
    fn test_complex_tree() {
        // Create a more complex tree structure with multiple levels
        let nodes = vec![
            // Root with multiple children
            (Nibbles::default(), create_test_node(&[], &[0x0, 0x5, 0xA, 0xF])),
            // Branch at 0x0 with children
            (Nibbles::from_nibbles([0x0]), create_test_node(&[], &[0x1, 0x2])),
            (Nibbles::from_nibbles([0x0, 0x1]), create_test_node(&[0x3], &[])),
            (Nibbles::from_nibbles([0x0, 0x2]), create_test_node(&[0x4], &[])),
            // Branch at 0x5 with no children (leaf)
            (Nibbles::from_nibbles([0x5]), create_test_node(&[0xB], &[])),
            // Branch at 0xA with deep nesting
            (Nibbles::from_nibbles([0xA]), create_test_node(&[], &[0xB])),
            (Nibbles::from_nibbles([0xA, 0xB]), create_test_node(&[], &[0xC])),
            (Nibbles::from_nibbles([0xA, 0xB, 0xC]), create_test_node(&[0xD], &[])),
            // Branch at 0xF (leaf)
            (Nibbles::from_nibbles([0xF]), create_test_node(&[0xE], &[])),
        ];

        let nodes_map: BTreeMap<_, _> = nodes.into_iter().collect();
        let factory = MockTrieCursorFactory::new(nodes_map, Default::default());
        let cursor = factory.account_trie_cursor().unwrap();
        let iter = DepthFirstTrieIterator::new(cursor);

        // Verify post-order traversal (children before parents)
        let expected_order = vec![
            Nibbles::from_nibbles([0x0, 0x1]),      // leaf child of 0x0
            Nibbles::from_nibbles([0x0, 0x2]),      // leaf child of 0x0
            Nibbles::from_nibbles([0x0]),           // parent of 0x01 and 0x02
            Nibbles::from_nibbles([0x5]),           // leaf
            Nibbles::from_nibbles([0xA, 0xB, 0xC]), // deepest leaf
            Nibbles::from_nibbles([0xA, 0xB]),      // parent of 0xABC
            Nibbles::from_nibbles([0xA]),           // parent of 0xAB
            Nibbles::from_nibbles([0xF]),           // leaf
            Nibbles::default(),                     // root (last)
        ];

        let mut actual_order = Vec::new();
        for result in iter {
            let (path, _node) = result.unwrap();
            actual_order.push(path);
        }

        assert_eq!(actual_order, expected_order);
    }
}
