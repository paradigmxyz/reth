use crate::{
    hashed_cursor::HashedCursorFactory,
    progress::{IntermediateStateRootState, StateRootProgress},
    trie::StateRoot,
    trie_cursor::{noop::NoopTrieCursorFactory, TrieCursor, TrieCursorFactory},
    Nibbles,
};
use alloy_primitives::{map::B256Map, B256};
use alloy_trie::BranchNodeCompact;
use reth_execution_errors::StateRootError;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::updates::StorageTrieUpdates;
use std::cmp::Ordering;

/// Used by [`StateRootBranchNodesIter`] to iterate over branch nodes in a state root.
#[derive(Debug)]
enum BranchNode {
    Account(Nibbles, BranchNodeCompact),
    Storage(B256, Nibbles, BranchNodeCompact),
}

/// Iterates over branch nodes produced by a [`StateRoot`]. The `StateRoot` will only used the
/// hashed accounts/storages tables, meaning it is recomputing the trie from scratch without the use
/// of the trie tables.
///
/// [`BranchNodes`] are iterated over such that:
/// * Account nodes and storage nodes may be interspersed.
/// * Storage nodes for the same account will be ordered by ascending path relative to each other.
/// * Account ndoes will be ordered by ascending path relative to each other.
/// * All storage nodes for one account will finish before storage nodes for another account are
///   started. In other words, if the current storage account is not equal to the previous, the
///   previous has no more nodes.
#[derive(Debug)]
struct StateRootBranchNodesIter<H> {
    hashed_cursor_factory: H,
    account_nodes: Vec<(Nibbles, BranchNodeCompact)>,
    storage_tries: B256Map<StorageTrieUpdates>,
    curr_storage: Option<(B256, Vec<(Nibbles, BranchNodeCompact)>)>,
    intermediate_state: Option<Box<IntermediateStateRootState>>,
    complete: bool,
}

impl<H> StateRootBranchNodesIter<H> {
    fn new(hashed_cursor_factory: H) -> Self {
        Self {
            hashed_cursor_factory,
            account_nodes: Default::default(),
            storage_tries: Default::default(),
            curr_storage: None,
            intermediate_state: None,
            complete: false,
        }
    }
}

impl<H: HashedCursorFactory + Clone> Iterator for StateRootBranchNodesIter<H> {
    type Item = Result<BranchNode, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // If we already started iterating through a storage trie's updates, continue doing
            // so.
            if let Some((account, storage_updates)) = self.curr_storage.as_mut() {
                let (path, node) = storage_updates.pop().expect("updates aren't empty");
                let node = BranchNode::Storage(*account, path, node);

                if storage_updates.is_empty() {
                    self.curr_storage = None;
                }

                return Some(Ok(node))
            }

            // If there's not a storage trie already being iterated over than check if there's a
            // storage trie we could start iterating over.
            if let Some(account) = self
                .storage_tries
                .iter()
                .filter_map(|(account, t)| (!t.storage_nodes.is_empty()).then_some(account))
                .copied()
                .next()
            {
                let mut storage_updates: Vec<_> = self
                    .storage_tries
                    .remove(&account)
                    .expect("account exists")
                    .storage_nodes
                    .into_iter()
                    .collect();
                debug_assert!(!storage_updates.is_empty());

                // sort nodes in reverse order, so that when they are popped off the Vec they are
                // popped in ascending order.
                storage_updates.sort_unstable_by(|a, b| a.0.cmp(&b.0).reverse());

                // reverse the `storage_updates` so that when we pop updates off of it later
                // they will be in the original order.
                storage_updates.reverse();

                self.curr_storage = Some((account, storage_updates));
                continue;
            }

            // `storage_updates` is empty, check if there are account updates.
            if let Some((path, node)) = self.account_nodes.pop() {
                return Some(Ok(BranchNode::Account(path, node)))
            }

            // All data from any previous runs of the `StateRoot` has been produced, run the next
            // partial computation, unless `StateRootProgress::Complete` has been returned in which
            // case iteration is over.
            if self.complete {
                return None
            }

            let state_root =
                StateRoot::new(NoopTrieCursorFactory, self.hashed_cursor_factory.clone())
                    .with_intermediate_state(self.intermediate_state.take().map(|s| *s));

            let updates = match state_root.root_with_progress() {
                Err(err) => return Some(Err(err)),
                Ok(StateRootProgress::Complete(_, _, updates)) => {
                    self.complete = true;
                    updates
                }
                Ok(StateRootProgress::Progress(intermediate_state, _, updates)) => {
                    self.intermediate_state = Some(intermediate_state);
                    updates
                }
            };

            // collect account updates and sort them in descending order, so that when we pop them
            // off the Vec they are popped in ascending order.
            self.account_nodes.extend(updates.account_nodes);
            self.account_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0).reverse());

            self.storage_tries = updates.storage_tries;

            // loop back to the top.
        }
    }
}

/// Inconsistency describes an inconsistency found when comparing the hashed state tables
/// ([`HashedCursorFactory`]) with that of the trie tables ([`TrieCursorFactory`]). The hashed
/// tables are considered the source of truth; inconsistencies are on the part of the trie tables.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Inconsistency {
    /// An extra account node was found.
    AccountExtra(Nibbles, BranchNodeCompact),
    /// A extra storage node was found.
    StorageExtra(B256, Nibbles, BranchNodeCompact),
    /// An account node had the wrong value.
    AccountWrong {
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// A storage node had the wrong value.
    StorageWrong {
        /// The account the storage trie belongs to.
        account: B256,
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// An account node was missing.
    AccountMissing(Nibbles, BranchNodeCompact),
    /// A storage node was missing.
    StorageMissing(B256, Nibbles, BranchNodeCompact),
}

/// Verifies the contents of a trie table against some other data source which is able to produce
/// stored trie nodes.
#[derive(Debug)]
struct SingleVerifier<C> {
    account: Option<B256>, // None for accounts trie
    trie_cursor: C,
    curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> SingleVerifier<C> {
    fn new(account: Option<B256>, mut trie_cursor: C) -> Result<Self, DatabaseError> {
        let curr = trie_cursor.seek(Nibbles::default())?;
        Ok(Self { account, trie_cursor, curr })
    }

    const fn inconsistency_extra(&self, path: Nibbles, node: BranchNodeCompact) -> Inconsistency {
        if let Some(account) = self.account {
            Inconsistency::StorageExtra(account, path, node)
        } else {
            Inconsistency::AccountExtra(path, node)
        }
    }

    const fn inconsistency_wrong(
        &self,
        path: Nibbles,
        expected: BranchNodeCompact,
        found: BranchNodeCompact,
    ) -> Inconsistency {
        if let Some(account) = self.account {
            Inconsistency::StorageWrong { account, path, expected, found }
        } else {
            Inconsistency::AccountWrong { path, expected, found }
        }
    }

    const fn inconsistency_missing(&self, path: Nibbles, node: BranchNodeCompact) -> Inconsistency {
        if let Some(account) = self.account {
            Inconsistency::StorageMissing(account, path, node)
        } else {
            Inconsistency::AccountMissing(path, node)
        }
    }

    /// Called with the next path and node in the canonical sequence of stored trie nodes. Will
    /// append to the given `inconsistencies` Vec if walking the trie cursor produces data
    /// inconsistent with that given.
    ///
    /// `next` must be called with paths in ascending order.
    fn next(
        &mut self,
        inconsistencies: &mut Vec<Inconsistency>,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), DatabaseError> {
        loop {
            // `curr` is None only if the end of the cursor has been reached. Any further nodes
            // found must be considered missing.
            if self.curr.is_none() {
                inconsistencies.push(self.inconsistency_missing(path, node));
                return Ok(())
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");

            match path.cmp(curr_path) {
                Ordering::Less => {
                    // If the given path is prior to the cursor's current path, then the given path
                    // was not produced by the cursor.
                    inconsistencies.push(self.inconsistency_missing(path, node));
                    return Ok(())
                }
                Ordering::Equal => {
                    // If the the current path matches the given one (happy path) but the nodes
                    // aren't equal then we produce a wrong node. Either way we want to move the
                    // trie cursor forward.
                    if *curr_node != node {
                        inconsistencies.push(self.inconsistency_wrong(
                            path,
                            node,
                            curr_node.clone(),
                        ))
                    }
                    self.curr = self.trie_cursor.next()?;
                    return Ok(())
                }
                Ordering::Greater => {
                    // If the current path is less than the given path it means the cursor's path
                    // was not found by the caller (otherwise it would have hit
                    // the equal case) and so is extraneous.
                    inconsistencies.push(self.inconsistency_extra(*curr_path, curr_node.clone()));
                    self.curr = self.trie_cursor.next()?;
                    // back to the top of the loop to check the latest `self.curr` value against the
                    // given path/node.
                }
            }
        }
    }

    /// Must be called once there are no more calls to `next` to made. All further nodes produced
    /// by the `TrieCursor` will be considered extraneous.
    fn finalize(&mut self, inconsistencies: &mut Vec<Inconsistency>) -> Result<(), DatabaseError> {
        loop {
            if let Some((curr_path, curr_node)) = self.curr.take() {
                inconsistencies.push(self.inconsistency_extra(curr_path, curr_node));
                self.curr = self.trie_cursor.next()?;
            } else {
                return Ok(())
            }
        }
    }
}

/// Checks that data stored in the trie database is consistent, using hashed accounts/storages
/// database tables as the source of truth. This will iteratively re-compute the entire trie based
/// on the hashed state, and produce any discovered [`Inconsistency`]s via the `next` method.
#[derive(Debug)]
pub struct Verifier<T: TrieCursorFactory, H> {
    trie_cursor_factory: T,
    branch_node_iter: StateRootBranchNodesIter<H>,
    inconsistencies: Vec<Inconsistency>,
    account: SingleVerifier<T::AccountTrieCursor>,
    storage: Option<(B256, SingleVerifier<T::StorageTrieCursor>)>,
    complete: bool,
}

impl<T: TrieCursorFactory + Clone, H: HashedCursorFactory + Clone> Verifier<T, H> {
    /// Creates a new verifier instance.
    pub fn new(trie_cursor_factory: T, hashed_cursor_factory: H) -> Result<Self, DatabaseError> {
        Ok(Self {
            trie_cursor_factory: trie_cursor_factory.clone(),
            branch_node_iter: StateRootBranchNodesIter::new(hashed_cursor_factory),
            inconsistencies: Default::default(),
            account: SingleVerifier::new(None, trie_cursor_factory.account_trie_cursor()?)?,
            storage: None,
            complete: false,
        })
    }
}

impl<T: TrieCursorFactory, H: HashedCursorFactory + Clone> Verifier<T, H> {
    fn new_storage(
        &mut self,
        account: B256,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), DatabaseError> {
        let trie_cursor = self.trie_cursor_factory.storage_trie_cursor(account)?;
        let mut storage = SingleVerifier::new(Some(account), trie_cursor)?;
        storage.next(&mut self.inconsistencies, path, node)?;
        self.storage = Some((account, storage));
        Ok(())
    }

    fn try_next(&mut self) -> Result<(), StateRootError> {
        match self.branch_node_iter.next().transpose()? {
            None => {
                self.account.finalize(&mut self.inconsistencies)?;
                if let Some((_, storage)) = self.storage.as_mut() {
                    storage.finalize(&mut self.inconsistencies)?;
                }
                self.complete = true;
            }
            Some(BranchNode::Account(path, node)) => {
                self.account.next(&mut self.inconsistencies, path, node)?
            }
            Some(BranchNode::Storage(account, path, node)) => match self.storage.as_mut() {
                None => self.new_storage(account, path, node)?,
                Some((prev_account, storage)) if *prev_account == account => {
                    storage.next(&mut self.inconsistencies, path, node)?;
                }
                Some((_, storage)) => {
                    storage.finalize(&mut self.inconsistencies)?;
                    self.new_storage(account, path, node)?;
                }
            },
        }

        // If any inconsistencies were appended we want to reverse them, so they are popped off
        // in the same order they were appended.
        self.inconsistencies.reverse();
        Ok(())
    }
}

impl<T: TrieCursorFactory, H: HashedCursorFactory + Clone> Iterator for Verifier<T, H> {
    type Item = Result<Inconsistency, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(inconsistency) = self.inconsistencies.pop() {
                return Some(Ok(inconsistency))
            }

            if self.complete {
                return None
            }

            if let Err(err) = self.try_next() {
                return Some(Err(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::mock::MockHashedCursorFactory,
        trie_cursor::mock::{MockTrieCursor, MockTrieCursorFactory},
    };
    use alloy_primitives::{address, keccak256, U256};
    use alloy_trie::TrieMask;
    use assert_matches::assert_matches;
    use reth_primitives_traits::Account;
    use std::collections::BTreeMap;

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
            final_hashes.push(B256::from([counter; 32]));
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
    fn test_state_root_branch_nodes_iter_empty() {
        // Test with completely empty state
        let factory = MockHashedCursorFactory::new(BTreeMap::new(), B256Map::default());
        let mut iter = StateRootBranchNodesIter::new(factory);

        // Collect all results - with empty state, should complete without producing nodes
        let mut count = 0;
        for result in iter.by_ref() {
            assert!(result.is_ok(), "Unexpected error: {:?}", result.unwrap_err());
            count += 1;
            // Prevent infinite loop in test
            assert!(count <= 1000, "Too many iterations");
        }

        assert!(iter.complete);
    }

    #[test]
    fn test_state_root_branch_nodes_iter_basic() {
        // Simple test with a few accounts and storage
        let mut accounts = BTreeMap::new();
        let mut storage_tries = B256Map::default();

        // Create test accounts
        let addr1 = keccak256(address!("0000000000000000000000000000000000000001"));
        accounts.insert(
            addr1,
            Account {
                nonce: 1,
                balance: U256::from(1000),
                bytecode_hash: Some(keccak256(b"code1")),
            },
        );

        // Add storage for the account
        let mut storage1 = BTreeMap::new();
        storage1.insert(keccak256(B256::from(U256::from(1))), U256::from(100));
        storage1.insert(keccak256(B256::from(U256::from(2))), U256::from(200));
        storage_tries.insert(addr1, storage1);

        let factory = MockHashedCursorFactory::new(accounts, storage_tries);
        let mut iter = StateRootBranchNodesIter::new(factory);

        // Collect nodes and verify basic properties
        let mut account_paths = Vec::new();
        let mut storage_paths_by_account: B256Map<Vec<Nibbles>> = B256Map::default();
        let mut iterations = 0;

        for result in iter.by_ref() {
            iterations += 1;
            assert!(iterations <= 10000, "Too many iterations - possible infinite loop");

            match result {
                Ok(BranchNode::Account(path, _)) => {
                    account_paths.push(path);
                }
                Ok(BranchNode::Storage(account, path, _)) => {
                    storage_paths_by_account.entry(account).or_default().push(path);
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        // Verify account paths are in ascending order
        for i in 1..account_paths.len() {
            assert!(
                account_paths[i - 1] < account_paths[i],
                "Account paths should be in ascending order"
            );
        }

        // Verify storage paths for each account are in ascending order
        for (account, paths) in storage_paths_by_account {
            for i in 1..paths.len() {
                assert!(
                    paths[i - 1] < paths[i],
                    "Storage paths for account {:?} should be in ascending order",
                    account
                );
            }
        }

        assert!(iter.complete);
    }

    #[test]
    fn test_state_root_branch_nodes_iter_multiple_accounts() {
        // Test with multiple accounts to verify ordering
        let mut accounts = BTreeMap::new();
        let mut storage_tries = B256Map::default();

        // Create multiple test addresses
        for i in 1u8..=3 {
            let addr = keccak256([i; 20]);
            accounts.insert(
                addr,
                Account {
                    nonce: i as u64,
                    balance: U256::from(i as u64 * 1000),
                    bytecode_hash: (i == 2).then(|| keccak256([i])),
                },
            );

            // Add some storage for each account
            let mut storage = BTreeMap::new();
            for j in 0..i {
                storage.insert(keccak256(B256::from(U256::from(j))), U256::from(j as u64 * 10));
            }
            if !storage.is_empty() {
                storage_tries.insert(addr, storage);
            }
        }

        let factory = MockHashedCursorFactory::new(accounts, storage_tries);
        let mut iter = StateRootBranchNodesIter::new(factory);

        // Track what we see
        let mut seen_storage_accounts = Vec::new();
        let mut current_storage_account = None;
        let mut iterations = 0;

        for result in iter.by_ref() {
            iterations += 1;
            assert!(iterations <= 10000, "Too many iterations");

            match result {
                Ok(BranchNode::Storage(account, _, _)) => {
                    if current_storage_account != Some(account) {
                        // Verify we don't revisit a storage account
                        assert!(
                            !seen_storage_accounts.contains(&account),
                            "Should not revisit storage account {:?}",
                            account
                        );
                        seen_storage_accounts.push(account);
                        current_storage_account = Some(account);
                    }
                }
                Ok(BranchNode::Account(_, _)) => {
                    // Account nodes are fine
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(iter.complete);
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
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node2 = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Call next with the exact node that exists
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have no inconsistencies
        assert!(inconsistencies.is_empty());
    }

    #[test]
    fn test_single_verifier_next_wrong_value() {
        // Test when the path matches but value is different
        let node_in_trie = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node_expected = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node_in_trie.clone())]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Call next with different node value
        verifier
            .next(&mut inconsistencies, Nibbles::from_nibbles([0x1]), node_expected.clone())
            .unwrap();

        // Should have one "wrong" inconsistency
        assert_eq!(inconsistencies.len(), 1);
        assert_matches!(
            &inconsistencies[0],
            Inconsistency::AccountWrong { path, expected, found }
                if *path == Nibbles::from_nibbles([0x1]) && *expected == node_expected && *found == node_in_trie
        );
    }

    #[test]
    fn test_single_verifier_next_missing() {
        // Test when expected node doesn't exist in trie
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node_missing = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x3]), node1)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Call next with a node that comes before any in the trie
        verifier
            .next(&mut inconsistencies, Nibbles::from_nibbles([0x1]), node_missing.clone())
            .unwrap();

        // Should have one "missing" inconsistency
        assert_eq!(inconsistencies.len(), 1);
        assert_matches!(
            &inconsistencies[0],
            Inconsistency::AccountMissing(path, node)
                if *path == Nibbles::from_nibbles([0x1]) && *node == node_missing
        );
    }

    #[test]
    fn test_single_verifier_next_extra() {
        // Test when trie has extra nodes not in expected
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node2 = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);
        let node3 = test_branch_node(0b1010, 0b0010, 0b1000, vec![B256::from([3u8; 32])]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x3]), node3.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Skip over node at path 0x1 and 0x2, go straight to 0x3
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x3]), node3).unwrap();

        // Should have two "extra" inconsistencies for the skipped nodes
        assert_eq!(inconsistencies.len(), 2);
        assert_matches!(
            &inconsistencies[0],
            Inconsistency::AccountExtra(path, node)
                if *path == Nibbles::from_nibbles([0x1]) && *node == node1
        );
        assert_matches!(
            &inconsistencies[1],
            Inconsistency::AccountExtra(path, node)
                if *path == Nibbles::from_nibbles([0x2]) && *node == node2
        );
    }

    #[test]
    fn test_single_verifier_finalize() {
        // Test finalize marks all remaining nodes as extra
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node2 = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);
        let node3 = test_branch_node(0b1010, 0b0010, 0b1000, vec![B256::from([3u8; 32])]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2),
            (Nibbles::from_nibbles([0x3]), node3),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Process first node correctly
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x1]), node1).unwrap();
        assert!(inconsistencies.is_empty());

        // Finalize - should mark remaining nodes as extra
        verifier.finalize(&mut inconsistencies).unwrap();

        // Should have two extra nodes
        assert_eq!(inconsistencies.len(), 2);
        for inconsistency in &inconsistencies {
            assert!(matches!(inconsistency, Inconsistency::AccountExtra(_, _)));
        }
    }

    #[test]
    fn test_single_verifier_storage_trie() {
        // Test SingleVerifier for storage trie (with account set)
        let account = B256::from([42u8; 32]);
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(Some(account), cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Call next with missing node
        let missing_node = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);
        verifier
            .next(&mut inconsistencies, Nibbles::from_nibbles([0x0]), missing_node.clone())
            .unwrap();

        // Should produce StorageMissing, not AccountMissing
        assert_eq!(inconsistencies.len(), 1);
        assert_matches!(
            &inconsistencies[0],
            Inconsistency::StorageMissing(acc, path, node)
                if *acc == account && *path == Nibbles::from_nibbles([0x0]) && *node == missing_node
        );
    }

    #[test]
    fn test_single_verifier_empty_trie() {
        // Test with empty trie cursor
        let trie_nodes = BTreeMap::new();
        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Any node should be marked as missing
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x1]), node.clone()).unwrap();

        assert_eq!(inconsistencies.len(), 1);
        assert_matches!(
            &inconsistencies[0],
            Inconsistency::AccountMissing(path, n)
                if *path == Nibbles::from_nibbles([0x1]) && *n == node
        );
    }

    #[test]
    fn test_single_verifier_ordering() {
        // Test that nodes must be provided in ascending order
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node2 = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);
        let node3 = test_branch_node(0b1010, 0b0010, 0b1000, vec![B256::from([3u8; 32])]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x3]), node3.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut inconsistencies = Vec::new();

        // Process in correct order
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut inconsistencies, Nibbles::from_nibbles([0x3]), node3).unwrap();

        // All should match, no inconsistencies
        assert!(inconsistencies.is_empty());
    }
}
