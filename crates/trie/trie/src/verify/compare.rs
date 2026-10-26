//! Depth-first comparison of persisted trie nodes against recomputed nodes.

use super::Output;
use crate::{
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        TrieCursor,
    },
    Nibbles,
};
use alloy_primitives::B256;
use alloy_trie::BranchNodeCompact;
use reth_storage_errors::db::DatabaseError;
use std::cmp::Ordering;
use tracing::trace;

/// Verifies the contents of a trie table against some other data source which is able to produce
/// stored trie nodes.
#[derive(Debug)]
pub(super) struct SingleVerifier<I> {
    account: Option<B256>, // None for accounts trie
    trie_iter: I,
    curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> SingleVerifier<DepthFirstTrieIterator<C>> {
    pub(super) fn new(account: Option<B256>, trie_cursor: C) -> Result<Self, DatabaseError> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let curr = trie_iter.next().transpose()?;
        Ok(Self { account, trie_iter, curr })
    }
}

impl<I> SingleVerifier<I>
where
    I: Iterator<Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>>,
{
    const fn output_extra(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageExtra(account, path, node)
        } else {
            Output::AccountExtra(path, node)
        }
    }

    const fn output_wrong(
        &self,
        path: Nibbles,
        expected: BranchNodeCompact,
        found: BranchNodeCompact,
    ) -> Output {
        if let Some(account) = self.account {
            Output::StorageWrong { account, path, expected, found }
        } else {
            Output::AccountWrong { path, expected, found }
        }
    }

    const fn output_missing(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageMissing(account, path, node)
        } else {
            Output::AccountMissing(path, node)
        }
    }

    /// Called with the next path and node in the canonical sequence of stored trie nodes. Will
    /// emit findings immediately if walking the trie cursor produces inconsistent data.
    /// Returning `false` from the callback cancels comparison before advancing the cursor again.
    ///
    /// `next` must be called with paths in depth-first order. Returns `true` on completion and
    /// `false` on cancellation. The verifier must not be reused after cancellation.
    pub(super) fn next(
        &mut self,
        emit: &mut impl FnMut(Output) -> bool,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<bool, DatabaseError> {
        loop {
            // `curr` is None only if the end of the iterator has been reached. Any further nodes
            // found must be considered missing.
            if self.curr.is_none() {
                return Ok(emit(self.output_missing(path, node)))
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");
            trace!(target: "trie::verify", account=?self.account, ?curr_path, ?path, "Current cursor node");

            // Use depth-first ordering for comparison
            match depth_first::cmp(&path, curr_path) {
                Ordering::Less => {
                    // If the given path comes before the cursor's current path in depth-first
                    // order, then the given path was not produced by the cursor.
                    return Ok(emit(self.output_missing(path, node)))
                }
                Ordering::Equal => {
                    // If the current path matches the given one (happy path) but the nodes
                    // aren't equal then we produce a wrong node. Either way we want to move the
                    // iterator forward.
                    if *curr_node != node && !emit(self.output_wrong(path, node, curr_node.clone()))
                    {
                        return Ok(false)
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    return Ok(true);
                }
                Ordering::Greater => {
                    // If the given path comes after the current path in depth-first order,
                    // it means the cursor's path was not found by the caller (otherwise it would
                    // have hit the equal case) and so is extraneous.
                    if !emit(self.output_extra(*curr_path, curr_node.clone())) {
                        return Ok(false);
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    // back to the top of the loop to check the latest `self.curr` value against the
                    // given path/node.
                }
            }
        }
    }

    /// Must be called once there are no more calls to `next` to be made. All further nodes produced
    /// by the iterator will be considered extraneous and emitted individually. Returning `false`
    /// from the callback stops comparison immediately; the verifier must not be reused after
    /// cancellation. Returns `true` on completion and `false` on cancellation.
    pub(super) fn finalize(
        &mut self,
        emit: &mut impl FnMut(Output) -> bool,
    ) -> Result<bool, DatabaseError> {
        loop {
            if let Some((curr_path, curr_node)) = self.curr.take() {
                if !emit(self.output_extra(curr_path, curr_node)) {
                    return Ok(false);
                }
                self.curr = self.trie_iter.next().transpose()?;
            } else {
                return Ok(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::{
        mock::{MockTrieCursor, MockTrieCursorFactory},
        TrieCursorFactory,
    };
    use alloy_primitives::map::B256Map;
    use alloy_trie::TrieMask;
    use assert_matches::assert_matches;
    use std::{cell::Cell, collections::BTreeMap};

    /// Helper function to create a simple test `BranchNodeCompact`
    fn test_branch_node(
        state_mask: u16,
        tree_mask: u16,
        hash_mask: u16,
        hashes: Vec<B256>,
    ) -> BranchNodeCompact {
        // Ensure the number of hashes matches the number of bits set in hash_mask
        let expected_hashes = hash_mask.count_ones() as usize;
        let mut final_hashes = hashes;
        let mut counter = 100u8;
        while final_hashes.len() < expected_hashes {
            final_hashes.push(B256::repeat_byte(counter));
            counter += 1;
        }
        final_hashes.truncate(expected_hashes);

        BranchNodeCompact::new(
            TrieMask::new(state_mask),
            TrieMask::new(tree_mask),
            TrieMask::new(hash_mask),
            final_hashes,
            None,
        )
    }

    /// Helper function to create a simple test `MockTrieCursor`
    fn create_mock_cursor(trie_nodes: BTreeMap<Nibbles, BranchNodeCompact>) -> MockTrieCursor {
        let factory = MockTrieCursorFactory::new(trie_nodes, B256Map::default());
        factory.account_trie_cursor().unwrap()
    }

    #[test]
    fn test_single_verifier_new() {
        // Test creating a new SingleVerifier for account trie
        let trie_nodes = BTreeMap::from([(
            Nibbles::from_nibbles([0x1]),
            test_branch_node(0b1111, 0, 0, vec![]),
        )]);

        let cursor = create_mock_cursor(trie_nodes);
        let verifier = SingleVerifier::new(None, cursor).unwrap();

        // Should have seeked to the beginning and found the first node
        assert!(verifier.curr.is_some());
    }

    #[test]
    fn test_single_verifier_next_exact_match() {
        // Test when the expected node matches exactly
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node2 = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with the exact node that exists
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have no outputs
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_next_wrong_value() {
        // Test when the path matches but value is different
        let node_in_trie = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node_expected = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node_in_trie.clone())]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with different node value
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node_expected.clone())
            .unwrap();

        // Should have one "wrong" output
        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::AccountWrong { path, expected, found }
                if *path == Nibbles::from_nibbles([0x1]) && *expected == node_expected && *found == node_in_trie
        );
    }

    #[test]
    fn test_single_verifier_next_missing() {
        // Test when expected node doesn't exist in trie
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node_missing = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x3]), node1)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with a node that comes before any in the trie
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node_missing.clone())
            .unwrap();

        // Should have one "missing" output
        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::AccountMissing(path, node)
                if *path == Nibbles::from_nibbles([0x1]) && *node == node_missing
        );
    }

    #[test]
    fn test_single_verifier_next_extra() {
        // Test when trie has extra nodes not in expected
        // Create a proper trie structure with root
        let node_root = test_branch_node(0b1110, 0, 0b1110, vec![]); // root has children at 1, 2, 3
        let node1 = test_branch_node(0b0001, 0, 0b0001, vec![]);
        let node2 = test_branch_node(0b0010, 0, 0b0010, vec![]);
        let node3 = test_branch_node(0b0100, 0, 0b0100, vec![]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x3]), node3.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces in post-order: 0x1, 0x2, 0x3, root
        // We only provide 0x1 and 0x3, skipping 0x2 and root
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x3]), node3).unwrap();
        verifier.finalize(&mut collect(&mut outputs)).unwrap();

        // Should have two "extra" outputs for nodes in the trie that we skipped
        if outputs.len() != 2 {
            eprintln!("Expected 2 outputs, got {}:", outputs.len());
            for inc in &outputs {
                eprintln!("  {:?}", inc);
            }
        }
        assert_eq!(outputs.len(), 2);
        assert_matches!(
            &outputs[0],
            Output::AccountExtra(path, node)
                if *path == Nibbles::from_nibbles([0x2]) && *node == node2
        );
        assert_matches!(
            &outputs[1],
            Output::AccountExtra(path, node)
                if *path == Nibbles::new() && *node == node_root
        );
    }

    #[test]
    fn test_single_verifier_finalize() {
        // Test finalize marks all remaining nodes as extra
        let node_root = test_branch_node(0b1110, 0, 0b1110, vec![]); // root has children at 1, 2, 3
        let node1 = test_branch_node(0b0001, 0, 0b0001, vec![]);
        let node2 = test_branch_node(0b0010, 0, 0b0010, vec![]);
        let node3 = test_branch_node(0b0100, 0, 0b0100, vec![]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x3]), node3.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces in post-order: 0x1, 0x2, 0x3, root
        // Process first two nodes correctly
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x2]), node2).unwrap();
        assert!(outputs.is_empty());

        // Finalize - should mark remaining nodes (0x3 and root) as extra
        verifier.finalize(&mut collect(&mut outputs)).unwrap();

        // Should have two extra nodes
        assert_eq!(outputs.len(), 2);
        assert_matches!(
            &outputs[0],
            Output::AccountExtra(path, node)
                if *path == Nibbles::from_nibbles([0x3]) && *node == node3
        );
        assert_matches!(
            &outputs[1],
            Output::AccountExtra(path, node)
                if *path == Nibbles::new() && *node == node_root
        );
    }

    #[test]
    fn test_single_verifier_storage_trie() {
        // Test SingleVerifier for storage trie (with account set)
        let account = B256::repeat_byte(42u8);
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(Some(account), cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with missing node
        let missing_node = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x0]), missing_node.clone())
            .unwrap();

        // Should produce StorageMissing, not AccountMissing
        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::StorageMissing(acc, path, node)
                if *acc == account && *path == Nibbles::from_nibbles([0x0]) && *node == missing_node
        );
    }

    #[test]
    fn test_single_verifier_empty_trie() {
        // Test with empty trie cursor
        let trie_nodes = BTreeMap::new();
        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Any node should be marked as missing
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node.clone())
            .unwrap();

        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::AccountMissing(path, n)
                if *path == Nibbles::from_nibbles([0x1]) && *n == node
        );
    }

    #[test]
    fn test_single_verifier_depth_first_ordering() {
        // Test that nodes must be provided in depth-first order
        // Create nodes with proper parent-child relationships
        let node_root = test_branch_node(0b0110, 0, 0b0110, vec![]); // root has children at 1 and 2
        let node1 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x1 has children at 1 and 2
        let node11 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x11 is a leaf
        let node12 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x12 is a leaf
        let node2 = test_branch_node(0b0100, 0, 0b0100, vec![]); // 0x2 is a leaf

        // The depth-first iterator will iterate from the root in this order:
        // root -> 0x1 -> 0x11, 0x12 (children of 0x1), then 0x2
        // But because of depth-first, we get: root, 0x1, 0x11, 0x12, 0x2
        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),                 // root
            (Nibbles::from_nibbles([0x1]), node1.clone()),       // 0x1
            (Nibbles::from_nibbles([0x1, 0x1]), node11.clone()), // 0x11
            (Nibbles::from_nibbles([0x1, 0x2]), node12.clone()), // 0x12
            (Nibbles::from_nibbles([0x2]), node2.clone()),       // 0x2
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces nodes in post-order (children before parents)
        // Order: 0x11, 0x12, 0x1, 0x2, root
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x1]), node11)
            .unwrap();
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x2]), node12)
            .unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::new(), node_root).unwrap();
        verifier.finalize(&mut collect(&mut outputs)).unwrap();

        // All should match, no outputs
        if !outputs.is_empty() {
            eprintln!(
                "Test test_single_verifier_depth_first_ordering failed with {} outputs:",
                outputs.len()
            );
            for inc in &outputs {
                eprintln!("  {:?}", inc);
            }
        }
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_wrong_depth_first_order() {
        // Test that providing nodes in wrong order produces outputs
        // Create a trie with parent-child relationship
        let node_root = test_branch_node(0b0010, 0, 0b0010, vec![]); // root has child at 1
        let node1 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x1 has child at 1
        let node11 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x11 is a leaf

        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x1, 0x1]), node11.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Process in WRONG order (skip root, provide child before processing all nodes correctly)
        // The iterator will produce: root, 0x1, 0x11
        // But we provide: 0x11, root, 0x1 (completely wrong order)
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x1]), node11)
            .unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::new(), node_root).unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have outputs since we provided them in wrong order
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_complex_depth_first() {
        // Test a complex tree structure with depth-first ordering
        // Build a tree structure with proper parent-child relationships
        let node_root = test_branch_node(0b0110, 0, 0b0110, vec![]); // root: children at nibbles 1
                                                                     // and 2
        let node1 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x1: children at nibbles 1 and 2
        let node11 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x11: children at nibbles 1 and
                                                                  // 2
        let node111 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x111: leaf
        let node112 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x112: leaf
        let node12 = test_branch_node(0b0100, 0, 0b0100, vec![]); // 0x12: leaf
        let node2 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x2: child at nibble 1
        let node21 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x21: leaf

        // Create the trie structure
        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x1, 0x1]), node11.clone()),
            (Nibbles::from_nibbles([0x1, 0x1, 0x1]), node111.clone()),
            (Nibbles::from_nibbles([0x1, 0x1, 0x2]), node112.clone()),
            (Nibbles::from_nibbles([0x1, 0x2]), node12.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x2, 0x1]), node21.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces nodes in post-order (children before parents)
        // Order: 0x111, 0x112, 0x11, 0x12, 0x1, 0x21, 0x2, root
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x1, 0x1]), node111)
            .unwrap();
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x1, 0x2]), node112)
            .unwrap();
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x1]), node11)
            .unwrap();
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1, 0x2]), node12)
            .unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier
            .next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x2, 0x1]), node21)
            .unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut collect(&mut outputs), Nibbles::new(), node_root).unwrap();
        verifier.finalize(&mut collect(&mut outputs)).unwrap();

        // All should match, no outputs
        if !outputs.is_empty() {
            eprintln!(
                "Test test_single_verifier_complex_depth_first failed with {} outputs:",
                outputs.len()
            );
            for inc in &outputs {
                eprintln!("  {:?}", inc);
            }
        }
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_many_extras_stream_and_cancel() {
        // A lazy source ensures neither comparison path needs to retain findings.
        for finalize in [false, true] {
            for cancel_after in [None, Some(17)] {
                let read = Cell::new(0);
                let nodes = (0..100_000).map(|_| {
                    read.set(read.get() + 1);
                    Ok((Nibbles::from_nibbles([1]), test_branch_node(1, 0, 0, vec![])))
                });
                let mut verifier = SingleVerifier { account: None, trie_iter: nodes, curr: None };
                verifier.curr = verifier.trie_iter.next().transpose().unwrap();
                let mut emitted = 0;
                let mut emit = |output| {
                    if matches!(output, Output::AccountMissing(..)) {
                        assert!(!finalize);
                        assert_eq!(emitted, 100_000);
                        return true;
                    }
                    emitted += 1;
                    assert_matches!(output, Output::AccountExtra(..));
                    assert_eq!(read.get(), emitted);
                    cancel_after != Some(emitted)
                };
                let result = if finalize {
                    verifier.finalize(&mut emit)
                } else {
                    verifier.next(&mut emit, Nibbles::new(), test_branch_node(1, 0, 0, vec![]))
                };
                if let Some(limit) = cancel_after {
                    assert_matches!(result, Ok(false));
                    assert_eq!(emitted, limit);
                    assert_eq!(read.get(), limit);
                } else {
                    // `next` also emits the missing canonical root after exhausting extras.
                    assert_matches!(result, Ok(true));
                    assert_eq!(emitted, 100_000);
                }
            }
        }
    }

    #[test]
    fn test_missing_and_wrong_cancel_before_advancing() {
        for curr in [None, Some((Nibbles::new(), test_branch_node(1, 0, 0, vec![])))] {
            let mut verifier = SingleVerifier {
                account: None,
                trie_iter: std::iter::once(Err(DatabaseError::Other("must not read".into()))),
                curr,
            };
            let result =
                verifier.next(&mut |_| false, Nibbles::new(), test_branch_node(2, 0, 0, vec![]));
            assert_matches!(result, Ok(false));
        }
    }

    #[test]
    fn test_database_error_is_preserved() {
        for finalize in [false, true] {
            let mut verifier = SingleVerifier {
                account: None,
                trie_iter: std::iter::once(Err(DatabaseError::Other("cursor failed".into()))),
                curr: Some((Nibbles::from_nibbles([1]), test_branch_node(1, 0, 0, vec![]))),
            };
            let result = if finalize {
                verifier.finalize(&mut |_| true)
            } else {
                verifier.next(&mut |_| true, Nibbles::new(), test_branch_node(1, 0, 0, vec![]))
            };
            assert_matches!(result, Err(DatabaseError::Other(message)) if message == "cursor failed");
        }
    }

    fn collect(outputs: &mut Vec<Output>) -> impl FnMut(Output) -> bool + '_ {
        |output| {
            outputs.push(output);
            true
        }
    }
}
