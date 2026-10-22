//! Parallel verification of persisted account and storage tries.

use self::{compare::SingleVerifier, pipeline::StoragePipeline};
use crate::{
    hashed_cursor::{HashedCursor, HashedStorageCursor},
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        TrieCursor, TrieStorageCursor,
    },
    Nibbles,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::BranchNodeCompact;
use reth_execution_errors::StateRootError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use std::{collections::VecDeque, thread};

mod compare;
mod pipeline;
mod storage;
pub use storage::StorageWorker;

/// Checks trie tables against hashed state, recomputing storage roots on a fixed sync pool.
///
/// Cursors must read the same fixed database version. Keep their transactions alive until
/// verification finishes. The lookahead thread and storage workers run until EOF or drop.
/// Completed roots accumulate without a limit while account leaves are consumed in order.
/// Inconsistencies from every thread are delivered through the same queue.
#[derive(Debug)]
pub struct Verifier<T: TrieCursor, H, S> {
    accounts: H,
    storage: S,
    account: SingleVerifier<DepthFirstTrieIterator<T>>,
    hash_builder: crate::HashBuilder,
    started: bool,
    pipeline: StoragePipeline,
    outputs: VecDeque<Result<Output, StateRootError>>,
    complete: bool,
}

const ACCOUNT_BATCH_SIZE: usize = 16_384;
const ACCOUNT_UPDATE_THRESHOLD: usize = 1024;

impl<T, H, S> Verifier<T, H, S>
where
    T: TrieCursor,
    H: HashedCursor<Value = Account>,
    S: HashedStorageCursor<Value = U256>,
{
    /// Starts verification with pre-created account, lookahead, and storage cursors.
    /// All cursors must read the same database version, and their transactions must remain alive
    /// until verification ends. Each storage worker owns its transaction and two reusable cursors.
    /// `storage_workers` must not be empty. Dropping the verifier cancels and joins background
    /// work.
    pub fn new<L, W, C, TX>(
        trie: T,
        accounts: H,
        storage: S,
        lookahead: L,
        storage_workers: Vec<StorageWorker<W, C, TX>>,
    ) -> Result<Self, DatabaseError>
    where
        L: HashedCursor<Value = Account> + Send + 'static,
        W: TrieStorageCursor + Send + 'static,
        C: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        let account = SingleVerifier::new(None, trie)?;
        let pipeline = StoragePipeline::spawn(lookahead, storage_workers)?;
        Ok(Self {
            accounts,
            storage,
            account,
            hash_builder: crate::HashBuilder::default().with_updates(true),
            started: false,
            pipeline,
            outputs: Default::default(),
            complete: false,
        })
    }

    fn fill_batch(&mut self) -> Result<(), StateRootError> {
        let mut account_rlp = Vec::with_capacity(crate::TRIE_ACCOUNT_RLP_MAX_SIZE);
        for _ in 0..ACCOUNT_BATCH_SIZE {
            let next = if self.started {
                self.accounts.next()?
            } else {
                self.started = true;
                self.accounts.seek(B256::ZERO)?
            };
            let Some((address, account)) = next else { return self.finish() };
            self.pipeline.drain(&mut self.outputs)?;
            self.storage.set_hashed_address(address);
            let root = if self.storage.is_storage_empty()? {
                alloy_consensus::EMPTY_ROOT_HASH
            } else {
                self.pipeline.root(address, &mut self.outputs)?
            };
            account_rlp.clear();
            account.into_trie_account(root).encode(&mut account_rlp);
            self.hash_builder.add_leaf(Nibbles::unpack(address), &account_rlp);
            if self.hash_builder.updates_len() >= ACCOUNT_UPDATE_THRESHOLD {
                self.verify_account_updates()?;
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), StateRootError> {
        self.pipeline.finish();
        self.complete = true;
        self.hash_builder.root();
        self.verify_account_updates()?;
        let mut found = Vec::new();
        self.account.finalize(&mut found)?;
        self.pipeline.report(found, &mut self.outputs)
    }

    fn verify_account_updates(&mut self) -> Result<(), StateRootError> {
        let (builder, updates) = std::mem::take(&mut self.hash_builder).split();
        self.hash_builder = builder.with_updates(true);
        // The account trie root is calculated but never persisted.
        let mut nodes =
            updates.into_iter().filter(|(path, _)| !path.is_empty()).collect::<Vec<_>>();
        nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
        let mut found = Vec::new();
        for (path, node) in nodes {
            self.account.next(&mut found, path, node)?;
            found.push(Output::Progress(path));
        }
        self.pipeline.report(found, &mut self.outputs)
    }
}

impl<T, H, S> Iterator for Verifier<T, H, S>
where
    T: TrieCursor,
    H: HashedCursor<Value = Account>,
    S: HashedStorageCursor<Value = U256>,
{
    type Item = Result<Output, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(output) = self.outputs.pop_front() {
                return Some(output)
            }
            if self.complete {
                return None
            }
            if let Err(error) = self.fill_batch() {
                self.complete = true;
                self.pipeline.stop();
                self.outputs.push_back(Err(error));
            }
        }
    }
}

/// Output describes an inconsistency found when comparing the hashed state tables
/// ([`HashedCursor`]) with that of the trie tables ([`TrieCursor`]). The hashed
/// tables are considered the source of truth; outputs are on the part of the trie tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// An extra account node was found.
    AccountExtra(Nibbles, BranchNodeCompact),
    /// An extra storage node was found.
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

/// Defaults to the available CPU count, or one worker if it cannot be determined.
pub fn default_storage_workers() -> usize {
    thread::available_parallelism().map_or(1, usize::from)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
        trie_cursor::{
            mock::{MockTrieCursor, MockTrieCursorFactory},
            noop::NoopTrieCursorFactory,
            TrieCursorFactory,
        },
    };
    use alloy_primitives::{keccak256, map::B256Map, Address, U256};
    use alloy_trie::TrieMask;
    use assert_matches::assert_matches;
    use reth_primitives_traits::Account;
    use std::{collections::BTreeMap, iter};

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

    fn verified_trie(factory: &MockHashedCursorFactory) -> MockTrieCursorFactory {
        let (_, updates) = crate::StateRoot::new(NoopTrieCursorFactory, factory.clone())
            .root_with_updates()
            .unwrap();
        let mut storages = updates
            .storage_tries
            .into_iter()
            .map(|(address, updates)| {
                (address, updates.storage_nodes.into_iter().collect::<BTreeMap<_, _>>())
            })
            .collect::<B256Map<_>>();
        storages.entry(B256::ZERO).or_default();
        let mut cursor = factory.hashed_account_cursor().unwrap();
        let mut next = cursor.seek(B256::ZERO).unwrap();
        while let Some((address, _)) = next {
            storages.entry(address).or_default();
            next = cursor.next().unwrap();
        }
        MockTrieCursorFactory::new(updates.account_nodes.into_iter().collect(), storages)
    }

    #[test]
    fn test_verifier_empty() {
        // Test with completely empty state
        let factory = MockHashedCursorFactory::new(
            BTreeMap::new(),
            iter::once((B256::ZERO, BTreeMap::new())).collect(),
        );
        let trie = verified_trie(&factory);
        let workers = (0..default_storage_workers())
            .map(|_| StorageWorker {
                trie: trie.storage_trie_cursor(B256::ZERO).unwrap(),
                hashed: factory.hashed_storage_cursor(B256::ZERO).unwrap(),
                transaction: (),
            })
            .collect();
        let mut verifier = Verifier::new(
            trie.account_trie_cursor().unwrap(),
            factory.hashed_account_cursor().unwrap(),
            factory.hashed_storage_cursor(B256::ZERO).unwrap(),
            factory.hashed_account_cursor().unwrap(),
            workers,
        )
        .unwrap();
        for output in verifier.by_ref() {
            assert_matches!(output.unwrap(), Output::Progress(_));
        }
        assert!(verifier.complete);
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_verifier_basic() {
        // Simple test with a few accounts and storage
        let mut accounts = BTreeMap::new();
        let mut storage_tries = B256Map::default();

        // Create test accounts
        let addr1 = keccak256(Address::with_last_byte(1));
        accounts.insert(
            addr1,
            Account {
                nonce: 1,
                balance: U256::from(1000),
                bytecode_hash: Some(keccak256(b"code1")),
                ..Default::default()
            },
        );

        // Add storage for the account
        let mut storage1 = BTreeMap::new();
        storage1.insert(keccak256(B256::with_last_byte(1)), U256::from(100));
        storage1.insert(keccak256(B256::with_last_byte(2)), U256::from(200));
        storage_tries.insert(addr1, storage1);

        storage_tries.entry(B256::ZERO).or_default();
        let factory = MockHashedCursorFactory::new(accounts, storage_tries);
        let trie = verified_trie(&factory);
        let workers = (0..default_storage_workers())
            .map(|_| StorageWorker {
                trie: trie.storage_trie_cursor(B256::ZERO).unwrap(),
                hashed: factory.hashed_storage_cursor(B256::ZERO).unwrap(),
                transaction: (),
            })
            .collect();
        let mut verifier = Verifier::new(
            trie.account_trie_cursor().unwrap(),
            factory.hashed_account_cursor().unwrap(),
            factory.hashed_storage_cursor(B256::ZERO).unwrap(),
            factory.hashed_account_cursor().unwrap(),
            workers,
        )
        .unwrap();
        for output in verifier.by_ref() {
            assert_matches!(output.unwrap(), Output::Progress(_));
        }
        assert!(verifier.complete);
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_verifier_multiple_accounts() {
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
                    ..Default::default()
                },
            );

            // Add some storage for each account
            let mut storage = BTreeMap::new();
            for j in 0..i {
                storage.insert(keccak256(B256::with_last_byte(j)), U256::from(j as u64 * 10));
            }
            if !storage.is_empty() {
                storage_tries.insert(addr, storage);
            }
        }

        storage_tries.entry(B256::ZERO).or_default();
        let factory = MockHashedCursorFactory::new(accounts, storage_tries);
        let trie = verified_trie(&factory);
        let workers = (0..default_storage_workers())
            .map(|_| StorageWorker {
                trie: trie.storage_trie_cursor(B256::ZERO).unwrap(),
                hashed: factory.hashed_storage_cursor(B256::ZERO).unwrap(),
                transaction: (),
            })
            .collect();
        let mut verifier = Verifier::new(
            trie.account_trie_cursor().unwrap(),
            factory.hashed_account_cursor().unwrap(),
            factory.hashed_storage_cursor(B256::ZERO).unwrap(),
            factory.hashed_account_cursor().unwrap(),
            workers,
        )
        .unwrap();
        for output in verifier.by_ref() {
            assert_matches!(output.unwrap(), Output::Progress(_));
        }
        assert!(verifier.complete);
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
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();

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
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node_missing = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

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
        let account = B256::repeat_byte(42u8);
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(Some(account), cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with missing node
        let missing_node = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);
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
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
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
