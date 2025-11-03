use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    progress::{IntermediateStateRootState, StateRootProgress},
    trie::StateRoot,
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        noop::NoopTrieCursorFactory,
        TrieCursor, TrieCursorFactory,
    },
    Nibbles,
};
use alloy_primitives::B256;
use alloy_trie::BranchNodeCompact;
use reth_execution_errors::StateRootError;
use reth_storage_errors::db::DatabaseError;
use std::cmp::Ordering;
use tracing::trace;

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
/// [`BranchNode`]s are iterated over such that:
/// * Account nodes and storage nodes may be interleaved.
/// * Storage nodes for the same account will be ordered by ascending path relative to each other.
/// * Account nodes will be ordered by ascending path relative to each other.
/// * All storage nodes for one account will finish before storage nodes for another account are
///   started. In other words, if the current storage account is not equal to the previous, the
///   previous has no more nodes.
#[derive(Debug)]
struct StateRootBranchNodesIter<H> {
    hashed_cursor_factory: H,
    account_nodes: Vec<(Nibbles, BranchNodeCompact)>,
    storage_tries: Vec<(B256, Vec<(Nibbles, BranchNodeCompact)>)>,
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

    /// Sorts a Vec of updates such that it is ready to be yielded from the `next` method. We yield
    /// by popping off of the account/storage vecs, so we sort them in reverse order.
    ///
    /// Depth-first sorting is used because this is the order that the `HashBuilder` computes
    /// branch nodes internally, even if it produces them as `B256Map`s.
    fn sort_updates(updates: &mut [(Nibbles, BranchNodeCompact)]) {
        updates.sort_unstable_by(|a, b| depth_first::cmp(&b.0, &a.0));
    }
}

impl<H: HashedCursorFactory + Clone> Iterator for StateRootBranchNodesIter<H> {
    type Item = Result<BranchNode, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // If we already started iterating through a storage trie's updates, continue doing
            // so.
            if let Some((account, storage_updates)) = self.curr_storage.as_mut() &&
                let Some((path, node)) = storage_updates.pop()
            {
                let node = BranchNode::Storage(*account, path, node);
                return Some(Ok(node))
            }

            // If there's not a storage trie already being iterated over than check if there's a
            // storage trie we could start iterating over.
            if let Some((account, storage_updates)) = self.storage_tries.pop() {
                debug_assert!(!storage_updates.is_empty());

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
            Self::sort_updates(&mut self.account_nodes);

            self.storage_tries = updates
                .storage_tries
                .into_iter()
                .filter_map(|(account, t)| {
                    (!t.storage_nodes.is_empty()).then(|| {
                        let mut storage_nodes = t.storage_nodes.into_iter().collect::<Vec<_>>();
                        Self::sort_updates(&mut storage_nodes);
                        (account, storage_nodes)
                    })
                })
                .collect::<Vec<_>>();

            // `root_with_progress` will output storage updates ordered by their account hash. If
            // `root_with_progress` only returns a partial result then it will pick up with where
            // it left off in the storage trie on the next run.
            //
            // By sorting by the account we ensure that we continue with the partially processed
            // trie (the last of the previous run) first. We sort in reverse order because we pop
            // off of this Vec.
            self.storage_tries.sort_unstable_by(|a, b| b.0.cmp(&a.0));

            // loop back to the top.
        }
    }
}

/// Output describes an inconsistency found when comparing the hashed state tables
/// ([`HashedCursorFactory`]) with that of the trie tables ([`TrieCursorFactory`]). The hashed
/// tables are considered the source of truth; outputs are on the part of the trie tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
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
    /// Progress indicator with the last seen account path.
    Progress(Nibbles),
}

/// Verifies the contents of a trie table against some other data source which is able to produce
/// stored trie nodes.
#[derive(Debug)]
struct SingleVerifier<I> {
    account: Option<B256>, // None for accounts trie
    trie_iter: I,
    curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> SingleVerifier<DepthFirstTrieIterator<C>> {
    fn new(account: Option<B256>, trie_cursor: C) -> Result<Self, DatabaseError> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let curr = trie_iter.next().transpose()?;
        Ok(Self { account, trie_iter, curr })
    }

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
    /// append to the given `outputs` Vec if walking the trie cursor produces data
    /// inconsistent with that given.
    ///
    /// `next` must be called with paths in depth-first order.
    fn next(
        &mut self,
        outputs: &mut Vec<Output>,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), DatabaseError> {
        loop {
            // `curr` is None only if the end of the iterator has been reached. Any further nodes
            // found must be considered missing.
            if self.curr.is_none() {
                outputs.push(self.output_missing(path, node));
                return Ok(())
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");
            trace!(target: "trie::verify", account=?self.account, ?curr_path, ?path, "Current cursor node");

            // Use depth-first ordering for comparison
            match depth_first::cmp(&path, curr_path) {
                Ordering::Less => {
                    // If the given path comes before the cursor's current path in depth-first
                    // order, then the given path was not produced by the cursor.
                    outputs.push(self.output_missing(path, node));
                    return Ok(())
                }
                Ordering::Equal => {
                    // If the the current path matches the given one (happy path) but the nodes
                    // aren't equal then we produce a wrong node. Either way we want to move the
                    // iterator forward.
                    if *curr_node != node {
                        outputs.push(self.output_wrong(path, node, curr_node.clone()))
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    return Ok(())
                }
                Ordering::Greater => {
                    // If the given path comes after the current path in depth-first order,
                    // it means the cursor's path was not found by the caller (otherwise it would
                    // have hit the equal case) and so is extraneous.
                    outputs.push(self.output_extra(*curr_path, curr_node.clone()));
                    self.curr = self.trie_iter.next().transpose()?;
                    // back to the top of the loop to check the latest `self.curr` value against the
                    // given path/node.
                }
            }
        }
    }

    /// Must be called once there are no more calls to `next` to made. All further nodes produced
    /// by the iterator will be considered extraneous.
    fn finalize(&mut self, outputs: &mut Vec<Output>) -> Result<(), DatabaseError> {
        loop {
            if let Some((curr_path, curr_node)) = self.curr.take() {
                outputs.push(self.output_extra(curr_path, curr_node));
                self.curr = self.trie_iter.next().transpose()?;
            } else {
                return Ok(())
            }
        }
    }
}

/// Checks that data stored in the trie database is consistent, using hashed accounts/storages
/// database tables as the source of truth. This will iteratively re-compute the entire trie based
/// on the hashed state, and produce any discovered [`Output`]s via the `next` method.
#[derive(Debug)]
pub struct Verifier<T: TrieCursorFactory, H> {
    trie_cursor_factory: T,
    hashed_cursor_factory: H,
    branch_node_iter: StateRootBranchNodesIter<H>,
    outputs: Vec<Output>,
    account: SingleVerifier<DepthFirstTrieIterator<T::AccountTrieCursor>>,
    storage: Option<(B256, SingleVerifier<DepthFirstTrieIterator<T::StorageTrieCursor>>)>,
    complete: bool,
}

impl<T: TrieCursorFactory + Clone, H: HashedCursorFactory + Clone> Verifier<T, H> {
    /// Creates a new verifier instance.
    pub fn new(trie_cursor_factory: T, hashed_cursor_factory: H) -> Result<Self, DatabaseError> {
        Ok(Self {
            trie_cursor_factory: trie_cursor_factory.clone(),
            hashed_cursor_factory: hashed_cursor_factory.clone(),
            branch_node_iter: StateRootBranchNodesIter::new(hashed_cursor_factory),
            outputs: Default::default(),
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
        storage.next(&mut self.outputs, path, node)?;
        self.storage = Some((account, storage));
        Ok(())
    }

    /// This method is called using the account hashes at the boundary of [`BranchNode::Storage`]
    /// sequences, ie once the [`StateRootBranchNodesIter`] has begun yielding storage nodes for a
    /// different account than it was yielding previously. All accounts between the two should have
    /// empty storages.
    fn verify_empty_storages(
        &mut self,
        last_account: B256,
        next_account: B256,
        start_inclusive: bool,
        end_inclusive: bool,
    ) -> Result<(), DatabaseError> {
        let mut account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let mut account_seeked = false;

        if !start_inclusive {
            account_seeked = true;
            account_cursor.seek(last_account)?;
        }

        loop {
            let Some((curr_account, _)) = (if account_seeked {
                account_cursor.next()?
            } else {
                account_seeked = true;
                account_cursor.seek(last_account)?
            }) else {
                return Ok(())
            };

            if curr_account < next_account || (end_inclusive && curr_account == next_account) {
                trace!(target: "trie::verify", account = ?curr_account, "Verying account has empty storage");

                let mut storage_cursor =
                    self.trie_cursor_factory.storage_trie_cursor(curr_account)?;
                let mut seeked = false;
                while let Some((path, node)) = if seeked {
                    storage_cursor.next()?
                } else {
                    seeked = true;
                    storage_cursor.seek(Nibbles::new())?
                } {
                    self.outputs.push(Output::StorageExtra(curr_account, path, node));
                }
            } else {
                return Ok(())
            }
        }
    }

    fn try_next(&mut self) -> Result<(), StateRootError> {
        match self.branch_node_iter.next().transpose()? {
            None => {
                self.account.finalize(&mut self.outputs)?;
                if let Some((prev_account, storage)) = self.storage.as_mut() {
                    storage.finalize(&mut self.outputs)?;

                    // If there was a previous storage account, and it is the final one, then we
                    // need to validate that all accounts coming after it have empty storages.
                    let prev_account = *prev_account;

                    // Calculate the max possible account address.
                    let mut max_account = B256::ZERO;
                    max_account.reverse();

                    self.verify_empty_storages(prev_account, max_account, false, true)?;
                }
                self.complete = true;
            }
            Some(BranchNode::Account(path, node)) => {
                trace!(target: "trie::verify", ?path, "Account node from state root");
                self.account.next(&mut self.outputs, path, node)?;
                // Push progress indicator
                if !path.is_empty() {
                    self.outputs.push(Output::Progress(path));
                }
            }
            Some(BranchNode::Storage(account, path, node)) => {
                trace!(target: "trie::verify", ?account, ?path, "Storage node from state root");
                match self.storage.as_mut() {
                    None => {
                        // First storage account - check for any empty storages before it
                        self.verify_empty_storages(B256::ZERO, account, true, false)?;
                        self.new_storage(account, path, node)?;
                    }
                    Some((prev_account, storage)) if *prev_account == account => {
                        storage.next(&mut self.outputs, path, node)?;
                    }
                    Some((prev_account, storage)) => {
                        storage.finalize(&mut self.outputs)?;
                        // Clear any storage entries between the previous account and the new one
                        let prev_account = *prev_account;
                        self.verify_empty_storages(prev_account, account, false, false)?;
                        self.new_storage(account, path, node)?;
                    }
                }
            }
        }

        // If any outputs were appended we want to reverse them, so they are popped off
        // in the same order they were appended.
        self.outputs.reverse();
        Ok(())
    }
}

impl<T: TrieCursorFactory, H: HashedCursorFactory + Clone> Iterator for Verifier<T, H> {
    type Item = Result<Output, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(output) = self.outputs.pop() {
                return Some(Ok(output))
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
    use alloy_primitives::{address, keccak256, map::B256Map, U256};
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
        let mut outputs = Vec::new();

        // Call next with the exact node that exists
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have no outputs
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_next_wrong_value() {
        // Test when the path matches but value is different
        let node_in_trie = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node_expected = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node_in_trie.clone())]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with different node value
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node_expected.clone()).unwrap();

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
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        let node_missing = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x3]), node1)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with a node that comes before any in the trie
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node_missing.clone()).unwrap();

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
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x3]), node3).unwrap();
        verifier.finalize(&mut outputs).unwrap();

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
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2]), node2).unwrap();
        assert!(outputs.is_empty());

        // Finalize - should mark remaining nodes (0x3 and root) as extra
        verifier.finalize(&mut outputs).unwrap();

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
        let account = B256::from([42u8; 32]);
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(Some(account), cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with missing node
        let missing_node = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::from([2u8; 32])]);
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x0]), missing_node.clone()).unwrap();

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
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::from([1u8; 32])]);
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node.clone()).unwrap();

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
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1]), node11).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x2]), node12).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut outputs, Nibbles::new(), node_root).unwrap();
        verifier.finalize(&mut outputs).unwrap();

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
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1]), node11).unwrap();
        verifier.next(&mut outputs, Nibbles::new(), node_root).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have outputs since we provided them in wrong order
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_complex_depth_first() {
        // Test a complex tree structure with depth-first ordering
        // Build a tree structure with proper parent-child relationships
        let node_root = test_branch_node(0b0110, 0, 0b0110, vec![]); // root: children at nibbles 1 and 2
        let node1 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x1: children at nibbles 1 and 2
        let node11 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x11: children at nibbles 1 and 2
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
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1, 0x1]), node111).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1, 0x2]), node112).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1]), node11).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x2]), node12).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2, 0x1]), node21).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut outputs, Nibbles::new(), node_root).unwrap();
        verifier.finalize(&mut outputs).unwrap();

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
}
