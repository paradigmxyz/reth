//! Common test suite for `ExternalStorage` implementations.

#[cfg(test)]
mod tests {
    use crate::{
        in_memory::InMemoryExternalStorage,
        storage::{
            BlockStateDiff, ExternalHashedCursor, ExternalStorage, ExternalStorageError,
            ExternalTrieCursor,
        },
    };
    use alloy_primitives::{B256, U256};
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdates, BranchNodeCompact, HashedPostState, Nibbles, TrieMask};
    use std::sync::Arc;
    use test_case::test_case;

    /// Helper to create a simple test branch node
    fn create_test_branch() -> BranchNodeCompact {
        let mut state_mask = TrieMask::default();
        state_mask.set_bit(0);
        state_mask.set_bit(1);

        BranchNodeCompact {
            state_mask,
            tree_mask: TrieMask::default(),
            hash_mask: TrieMask::default(),
            hashes: Arc::new(vec![]),
            root_hash: None,
        }
    }

    /// Helper to create a variant test branch node for comparison tests
    fn create_test_branch_variant() -> BranchNodeCompact {
        let mut state_mask = TrieMask::default();
        state_mask.set_bit(5);
        state_mask.set_bit(6);

        BranchNodeCompact {
            state_mask,
            tree_mask: TrieMask::default(),
            hash_mask: TrieMask::default(),
            hashes: Arc::new(vec![]),
            root_hash: None,
        }
    }

    /// Helper to create nibbles from a vector of u8 values
    fn nibbles_from(vec: Vec<u8>) -> Nibbles {
        Nibbles::from_nibbles_unchecked(vec)
    }

    /// Helper to create a test account
    fn create_test_account() -> Account {
        Account {
            nonce: 42,
            balance: U256::from(1000000),
            bytecode_hash: Some(B256::repeat_byte(0xBB)),
        }
    }

    /// Helper to create a test account with custom values
    fn create_test_account_with_values(nonce: u64, balance: u64, code_hash_byte: u8) -> Account {
        Account {
            nonce,
            balance: U256::from(balance),
            bytecode_hash: Some(B256::repeat_byte(code_hash_byte)),
        }
    }

    /// Test basic storage and retrieval of earliest block number
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_earliest_block_operations<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        // Initially should be None
        let earliest = storage.get_earliest_block_number().await?;
        assert!(earliest.is_none());

        // Set earliest block
        let block_hash = B256::repeat_byte(0x42);
        storage.set_earliest_block_number(100, block_hash).await?;

        // Should retrieve the same values
        let earliest = storage.get_earliest_block_number().await?;
        assert_eq!(earliest, Some((100, block_hash)));

        Ok(())
    }

    /// Test storing and retrieving trie updates
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_trie_updates_operations<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let block_number = 50;
        let trie_updates = TrieUpdates::default();
        let post_state = HashedPostState::default();
        let block_state_diff =
            BlockStateDiff { trie_updates: trie_updates.clone(), post_state: post_state.clone() };

        // Store trie updates
        storage.store_trie_updates(block_number, block_state_diff).await?;

        // Retrieve and verify
        let retrieved_diff = storage.fetch_trie_updates(block_number).await?;
        assert_eq!(retrieved_diff.trie_updates, trie_updates);
        assert_eq!(retrieved_diff.post_state, post_state);

        Ok(())
    }

    // =============================================================================
    // 1. Basic Cursor Operations
    // =============================================================================

    /// Test cursor operations on empty trie
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_cursor_empty_trie<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let mut cursor = storage.trie_cursor(None, 100)?;

        // All operations should return None on empty trie
        assert!(cursor.seek_exact(Nibbles::default())?.is_none());
        assert!(cursor.seek(Nibbles::default())?.is_none());
        assert!(cursor.next()?.is_none());
        assert!(cursor.current()?.is_none());

        Ok(())
    }

    /// Test cursor operations with single entry
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_cursor_single_entry<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        // Store single entry
        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;

        // Test seek_exact
        let result = cursor.seek_exact(path)?.unwrap();
        assert_eq!(result.0, path);

        // Test current position
        assert_eq!(cursor.current()?.unwrap(), path);

        // Test next from end should return None
        assert!(cursor.next()?.is_none());

        Ok(())
    }

    /// Test cursor operations with multiple entries
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_cursor_multiple_entries<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let paths = vec![
            nibbles_from(vec![1]),
            nibbles_from(vec![1, 2]),
            nibbles_from(vec![2]),
            nibbles_from(vec![2, 3]),
        ];
        let branch = create_test_branch();

        // Store multiple entries
        for path in &paths {
            storage.store_account_branches(50, vec![(*path, Some(branch.clone()))]).await?;
        }

        let mut cursor = storage.trie_cursor(None, 100)?;

        // Test that we can iterate through all entries
        let mut found_paths = Vec::new();
        while let Some((path, _)) = cursor.next()? {
            found_paths.push(path);
        }

        assert_eq!(found_paths.len(), 4);
        // Paths should be in lexicographic order
        for i in 0..paths.len() {
            assert_eq!(found_paths[i], paths[i]);
        }

        Ok(())
    }

    // =============================================================================
    // 2. Seek Operations
    // =============================================================================

    /// Test `seek_exact` with existing path
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_exact_existing_path<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        let result = cursor.seek_exact(path)?.unwrap();
        assert_eq!(result.0, path);

        Ok(())
    }

    /// Test `seek_exact` with non-existing path
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_exact_non_existing_path<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        let non_existing = nibbles_from(vec![4, 5, 6]);
        assert!(cursor.seek_exact(non_existing)?.is_none());

        Ok(())
    }

    /// Test `seek_exact` with empty path
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_exact_empty_path<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        let result = cursor.seek_exact(Nibbles::default())?.unwrap();
        assert_eq!(result.0, Nibbles::default());

        Ok(())
    }

    /// Test seek to existing path
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_to_existing_path<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        let result = cursor.seek(path)?.unwrap();
        assert_eq!(result.0, path);

        Ok(())
    }

    /// Test seek between existing nodes
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_between_existing_nodes<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![3]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path1, Some(branch.clone()))]).await?;
        storage.store_account_branches(50, vec![(path2, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        // Seek to path between 1 and 3, should return path 3
        let seek_path = nibbles_from(vec![2]);
        let result = cursor.seek(seek_path)?.unwrap();
        assert_eq!(result.0, path2);

        Ok(())
    }

    /// Test seek after all nodes
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_after_all_nodes<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        // Seek to path after all nodes
        let seek_path = nibbles_from(vec![9]);
        assert!(cursor.seek(seek_path)?.is_none());

        Ok(())
    }

    /// Test seek before all nodes
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_seek_before_all_nodes<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![5]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        // Seek to path before all nodes, should return first node
        let seek_path = nibbles_from(vec![1]);
        let result = cursor.seek(seek_path)?.unwrap();
        assert_eq!(result.0, path);

        Ok(())
    }

    // =============================================================================
    // 3. Navigation Tests
    // =============================================================================

    /// Test next without prior seek
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_next_without_prior_seek<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        // next() without prior seek should start from beginning
        let result = cursor.next()?.unwrap();
        assert_eq!(result.0, path);

        Ok(())
    }

    /// Test next after seek
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_next_after_seek<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![2]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path1, Some(branch.clone()))]).await?;
        storage.store_account_branches(50, vec![(path2, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        cursor.seek(path1)?;

        // next() should return second node
        let result = cursor.next()?.unwrap();
        assert_eq!(result.0, path2);

        Ok(())
    }

    /// Test next at end of trie
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_next_at_end_of_trie<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;
        cursor.seek(path)?;

        // next() at end should return None
        assert!(cursor.next()?.is_none());

        Ok(())
    }

    /// Test multiple consecutive next calls
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_multiple_consecutive_next<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let paths = vec![nibbles_from(vec![1]), nibbles_from(vec![2]), nibbles_from(vec![3])];
        let branch = create_test_branch();

        for path in &paths {
            storage.store_account_branches(50, vec![(*path, Some(branch.clone()))]).await?;
        }

        let mut cursor = storage.trie_cursor(None, 100)?;

        // Iterate through all with consecutive next() calls
        for expected_path in &paths {
            let result = cursor.next()?.unwrap();
            assert_eq!(result.0, *expected_path);
        }

        // Final next() should return None
        assert!(cursor.next()?.is_none());

        Ok(())
    }

    /// Test current after operations
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_current_after_operations<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![2]);
        let branch = create_test_branch();

        storage.store_account_branches(50, vec![(path1, Some(branch.clone()))]).await?;
        storage.store_account_branches(50, vec![(path2, Some(branch.clone()))]).await?;

        let mut cursor = storage.trie_cursor(None, 100)?;

        // Current should be None initially
        assert!(cursor.current()?.is_none());

        // After seek, current should track position
        cursor.seek(path1)?;
        assert_eq!(cursor.current()?.unwrap(), path1);

        // After next, current should update
        cursor.next()?;
        assert_eq!(cursor.current()?.unwrap(), path2);

        Ok(())
    }

    /// Test current with no prior operations
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_current_no_prior_operations<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let mut cursor = storage.trie_cursor(None, 100)?;

        // Current should be None when no operations performed
        assert!(cursor.current()?.is_none());

        Ok(())
    }

    // =============================================================================
    // 4. Block Number Filtering
    // =============================================================================

    /// Test same path with different blocks
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_same_path_different_blocks<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2]);
        let branch1 = create_test_branch();
        let branch2 = create_test_branch_variant();

        // Store same path at different blocks
        storage.store_account_branches(50, vec![(path, Some(branch1.clone()))]).await?;
        storage.store_account_branches(100, vec![(path, Some(branch2.clone()))]).await?;

        // Cursor with max_block_number=75 should see only block 50 data
        let mut cursor75 = storage.trie_cursor(None, 75)?;
        let result75 = cursor75.seek_exact(path)?.unwrap();
        assert_eq!(result75.0, path);

        // Cursor with max_block_number=150 should see block 100 data (latest)
        let mut cursor150 = storage.trie_cursor(None, 150)?;
        let result150 = cursor150.seek_exact(path)?.unwrap();
        assert_eq!(result150.0, path);

        Ok(())
    }

    /// Test deleted branch nodes
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_deleted_branch_nodes<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2]);
        let branch = create_test_branch();

        // Store branch node, then delete it (store None)
        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;
        storage.store_account_branches(100, vec![(path, None)]).await?;

        // Cursor before deletion should see the node
        let mut cursor75 = storage.trie_cursor(None, 75)?;
        assert!(cursor75.seek_exact(path)?.is_some());

        // Cursor after deletion should not see the node
        let mut cursor150 = storage.trie_cursor(None, 150)?;
        assert!(cursor150.seek_exact(path)?.is_none());

        Ok(())
    }

    // =============================================================================
    // 5. Hashed Address Filtering
    // =============================================================================

    /// Test account-specific cursor
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_account_specific_cursor<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2]);
        let addr1 = B256::repeat_byte(0x01);
        let addr2 = B256::repeat_byte(0x02);
        let branch = create_test_branch();

        // Store same path for different accounts (using storage branches)
        storage.store_storage_branches(50, addr1, vec![(path, Some(branch.clone()))]).await?;
        storage.store_storage_branches(50, addr2, vec![(path, Some(branch.clone()))]).await?;

        // Cursor for addr1 should only see addr1 data
        let mut cursor1 = storage.trie_cursor(Some(addr1), 100)?;
        let result1 = cursor1.seek_exact(path)?.unwrap();
        assert_eq!(result1.0, path);

        // Cursor for addr2 should only see addr2 data
        let mut cursor2 = storage.trie_cursor(Some(addr2), 100)?;
        let result2 = cursor2.seek_exact(path)?.unwrap();
        assert_eq!(result2.0, path);

        // Cursor for addr1 should not see addr2 data when iterating
        let mut cursor1_iter = storage.trie_cursor(Some(addr1), 100)?;
        let mut found_count = 0;
        while cursor1_iter.next()?.is_some() {
            found_count += 1;
        }
        assert_eq!(found_count, 1); // Should only see one entry (for addr1)

        Ok(())
    }

    /// Test state trie cursor
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_state_trie_cursor<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path = nibbles_from(vec![1, 2]);
        let addr = B256::repeat_byte(0x01);
        let branch = create_test_branch();

        // Store data for account trie and state trie
        storage.store_storage_branches(50, addr, vec![(path, Some(branch.clone()))]).await?;
        storage.store_account_branches(50, vec![(path, Some(branch.clone()))]).await?;

        // State trie cursor (None address) should only see state trie data
        let mut state_cursor = storage.trie_cursor(None, 100)?;
        let result = state_cursor.seek_exact(path)?.unwrap();
        assert_eq!(result.0, path);

        // Verify state cursor doesn't see account data when iterating
        let mut state_cursor_iter = storage.trie_cursor(None, 100)?;
        let mut found_count = 0;
        while state_cursor_iter.next()?.is_some() {
            found_count += 1;
        }

        assert_eq!(found_count, 1); // Should only see state trie entry

        Ok(())
    }

    /// Test mixed account and state data
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_mixed_account_state_data<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![2]);
        let addr = B256::repeat_byte(0x01);
        let branch = create_test_branch();

        // Store mixed account and state trie data
        storage.store_storage_branches(50, addr, vec![(path1, Some(branch.clone()))]).await?;
        storage.store_account_branches(50, vec![(path2, Some(branch.clone()))]).await?;

        // Account cursor should only see account data
        let mut account_cursor = storage.trie_cursor(Some(addr), 100)?;
        let mut account_paths = Vec::new();
        while let Some((path, _)) = account_cursor.next()? {
            account_paths.push(path);
        }
        assert_eq!(account_paths.len(), 1);
        assert_eq!(account_paths[0], path1);

        // State cursor should only see state data
        let mut state_cursor = storage.trie_cursor(None, 100)?;
        let mut state_paths = Vec::new();
        while let Some((path, _)) = state_cursor.next()? {
            state_paths.push(path);
        }
        assert_eq!(state_paths.len(), 1);
        assert_eq!(state_paths[0], path2);

        Ok(())
    }

    // =============================================================================
    // 6. Path Ordering Tests
    // =============================================================================

    /// Test lexicographic ordering
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_lexicographic_ordering<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let paths = vec![
            nibbles_from(vec![3, 1]),
            nibbles_from(vec![1, 2]),
            nibbles_from(vec![2]),
            nibbles_from(vec![1]),
        ];
        let branch = create_test_branch();

        // Store paths in random order
        for path in &paths {
            storage.store_account_branches(50, vec![(*path, Some(branch.clone()))]).await?;
        }

        let mut cursor = storage.trie_cursor(None, 100)?;
        let mut found_paths = Vec::new();
        while let Some((path, _)) = cursor.next()? {
            found_paths.push(path);
        }

        // Should be returned in lexicographic order: [1], [1,2], [2], [3,1]
        let expected_order = vec![
            nibbles_from(vec![1]),
            nibbles_from(vec![1, 2]),
            nibbles_from(vec![2]),
            nibbles_from(vec![3, 1]),
        ];

        assert_eq!(found_paths, expected_order);

        Ok(())
    }

    /// Test path prefix scenarios
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_path_prefix_scenarios<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let paths = vec![
            nibbles_from(vec![1]),       // Prefix of next
            nibbles_from(vec![1, 2]),    // Extends first
            nibbles_from(vec![1, 2, 3]), // Extends second
        ];
        let branch = create_test_branch();

        for path in &paths {
            storage.store_account_branches(50, vec![(*path, Some(branch.clone()))]).await?;
        }

        let mut cursor = storage.trie_cursor(None, 100)?;

        // Seek to prefix should find exact match
        let result = cursor.seek_exact(paths[0])?.unwrap();
        assert_eq!(result.0, paths[0]);

        // Next should go to next path, not skip prefixed paths
        let result = cursor.next()?.unwrap();
        assert_eq!(result.0, paths[1]);

        let result = cursor.next()?.unwrap();
        assert_eq!(result.0, paths[2]);

        Ok(())
    }

    /// Test complex nibble combinations
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_complex_nibble_combinations<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        // Test various nibble patterns including edge values
        let paths = vec![
            nibbles_from(vec![0]),
            nibbles_from(vec![0, 15]),
            nibbles_from(vec![15]),
            nibbles_from(vec![15, 0]),
            nibbles_from(vec![7, 8, 9]),
        ];
        let branch = create_test_branch();

        for path in &paths {
            storage.store_account_branches(50, vec![(*path, Some(branch.clone()))]).await?;
        }

        let mut cursor = storage.trie_cursor(None, 100)?;
        let mut found_paths = Vec::new();
        while let Some((path, _)) = cursor.next()? {
            found_paths.push(path);
        }

        // All paths should be found and in correct order
        assert_eq!(found_paths.len(), 5);

        // Verify specific ordering for edge cases
        assert_eq!(found_paths[0], nibbles_from(vec![0]));
        assert_eq!(found_paths[1], nibbles_from(vec![0, 15]));
        assert_eq!(found_paths[4], nibbles_from(vec![15, 0]));

        Ok(())
    }

    // =============================================================================
    // 7. Leaf Node Tests (Hashed Accounts and Storage)
    // =============================================================================

    /// Test store and retrieve single account
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_store_and_retrieve_single_account<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let account_key = B256::repeat_byte(0x01);
        let account = create_test_account();

        // Store account
        storage.store_hashed_accounts(vec![(account_key, Some(account))], 50).await?;

        // Retrieve via cursor
        let mut cursor = storage.account_hashed_cursor(100)?;
        let result = cursor.seek(account_key)?.unwrap();

        assert_eq!(result.0, account_key);
        assert_eq!(result.1.nonce, account.nonce);
        assert_eq!(result.1.balance, account.balance);
        assert_eq!(result.1.bytecode_hash, account.bytecode_hash);

        Ok(())
    }

    /// Test account cursor navigation
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_account_cursor_navigation<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let accounts = [
            (B256::repeat_byte(0x01), create_test_account()),
            (B256::repeat_byte(0x03), create_test_account()),
            (B256::repeat_byte(0x05), create_test_account()),
        ];

        // Store accounts
        let accounts_to_store: Vec<_> = accounts.iter().map(|(k, v)| (*k, Some(*v))).collect();
        storage.store_hashed_accounts(accounts_to_store, 50).await?;

        let mut cursor = storage.account_hashed_cursor(100)?;

        // Test seeking to exact key
        let result = cursor.seek(accounts[1].0)?.unwrap();
        assert_eq!(result.0, accounts[1].0);

        // Test seeking to key that doesn't exist (should return next greater)
        let seek_key = B256::repeat_byte(0x02);
        let result = cursor.seek(seek_key)?.unwrap();
        assert_eq!(result.0, accounts[1].0); // Should find 0x03

        // Test next() navigation
        let result = cursor.next()?.unwrap();
        assert_eq!(result.0, accounts[2].0); // Should find 0x05

        // Test next() at end
        assert!(cursor.next()?.is_none());

        Ok(())
    }

    /// Test account block versioning
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_account_block_versioning<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let account_key = B256::repeat_byte(0x01);
        let account_v1 = create_test_account_with_values(1, 100, 0xBB);
        let account_v2 = create_test_account_with_values(2, 200, 0xDD);

        // Store account at different blocks
        storage.store_hashed_accounts(vec![(account_key, Some(account_v1))], 50).await?;
        storage.store_hashed_accounts(vec![(account_key, Some(account_v2))], 100).await?;

        // Cursor with max_block_number=75 should see v1
        let mut cursor75 = storage.account_hashed_cursor(75)?;
        let result75 = cursor75.seek(account_key)?.unwrap();
        assert_eq!(result75.1.nonce, account_v1.nonce);
        assert_eq!(result75.1.balance, account_v1.balance);

        // Cursor with max_block_number=150 should see v2
        let mut cursor150 = storage.account_hashed_cursor(150)?;
        let result150 = cursor150.seek(account_key)?.unwrap();
        assert_eq!(result150.1.nonce, account_v2.nonce);
        assert_eq!(result150.1.balance, account_v2.balance);

        Ok(())
    }

    /// Test store and retrieve storage
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_store_and_retrieve_storage<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let hashed_address = B256::repeat_byte(0x01);
        let storage_slots = vec![
            (B256::repeat_byte(0x10), U256::from(100)),
            (B256::repeat_byte(0x20), U256::from(200)),
            (B256::repeat_byte(0x30), U256::from(300)),
        ];

        // Store storage slots
        storage.store_hashed_storages(hashed_address, storage_slots.clone(), 50).await?;

        // Retrieve via cursor
        let mut cursor = storage.storage_hashed_cursor(hashed_address, 100)?;

        // Test seeking to each slot
        for (key, expected_value) in &storage_slots {
            let result = cursor.seek(*key)?.unwrap();
            assert_eq!(result.0, *key);
            assert_eq!(result.1, *expected_value);
        }

        Ok(())
    }

    /// Test storage cursor navigation
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_storage_cursor_navigation<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let hashed_address = B256::repeat_byte(0x01);
        let storage_slots = vec![
            (B256::repeat_byte(0x10), U256::from(100)),
            (B256::repeat_byte(0x30), U256::from(300)),
            (B256::repeat_byte(0x50), U256::from(500)),
        ];

        storage.store_hashed_storages(hashed_address, storage_slots.clone(), 50).await?;

        let mut cursor = storage.storage_hashed_cursor(hashed_address, 100)?;

        // Start from beginning with next()
        let mut found_slots = Vec::new();
        while let Some((key, value)) = cursor.next()? {
            found_slots.push((key, value));
        }

        assert_eq!(found_slots.len(), 3);
        assert_eq!(found_slots[0], storage_slots[0]);
        assert_eq!(found_slots[1], storage_slots[1]);
        assert_eq!(found_slots[2], storage_slots[2]);

        Ok(())
    }

    /// Test storage account isolation
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_storage_account_isolation<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let address1 = B256::repeat_byte(0x01);
        let address2 = B256::repeat_byte(0x02);
        let storage_key = B256::repeat_byte(0x10);

        // Store same storage key for different accounts
        storage.store_hashed_storages(address1, vec![(storage_key, U256::from(100))], 50).await?;
        storage.store_hashed_storages(address2, vec![(storage_key, U256::from(200))], 50).await?;

        // Verify each account sees only its own storage
        let mut cursor1 = storage.storage_hashed_cursor(address1, 100)?;
        let result1 = cursor1.seek(storage_key)?.unwrap();
        assert_eq!(result1.1, U256::from(100));

        let mut cursor2 = storage.storage_hashed_cursor(address2, 100)?;
        let result2 = cursor2.seek(storage_key)?.unwrap();
        assert_eq!(result2.1, U256::from(200));

        // Verify cursor1 doesn't see address2's storage
        let mut cursor1_iter = storage.storage_hashed_cursor(address1, 100)?;
        let mut count = 0;
        while cursor1_iter.next()?.is_some() {
            count += 1;
        }
        assert_eq!(count, 1); // Should only see one entry

        Ok(())
    }

    /// Test storage block versioning
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_storage_block_versioning<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store storage at different blocks
        storage
            .store_hashed_storages(hashed_address, vec![(storage_key, U256::from(100))], 50)
            .await?;
        storage
            .store_hashed_storages(hashed_address, vec![(storage_key, U256::from(200))], 100)
            .await?;

        // Cursor with max_block_number=75 should see old value
        let mut cursor75 = storage.storage_hashed_cursor(hashed_address, 75)?;
        let result75 = cursor75.seek(storage_key)?.unwrap();
        assert_eq!(result75.1, U256::from(100));

        // Cursor with max_block_number=150 should see new value
        let mut cursor150 = storage.storage_hashed_cursor(hashed_address, 150)?;
        let result150 = cursor150.seek(storage_key)?.unwrap();
        assert_eq!(result150.1, U256::from(200));

        Ok(())
    }

    /// Test storage zero value deletion
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_storage_zero_value_deletion<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store non-zero value
        storage
            .store_hashed_storages(hashed_address, vec![(storage_key, U256::from(100))], 50)
            .await?;

        // "Delete" by storing zero value
        storage.store_hashed_storages(hashed_address, vec![(storage_key, U256::ZERO)], 100).await?;

        // Cursor before deletion should see the value
        let mut cursor75 = storage.storage_hashed_cursor(hashed_address, 75)?;
        let result75 = cursor75.seek(storage_key)?.unwrap();
        assert_eq!(result75.1, U256::from(100));

        // Cursor after deletion should NOT see the entry (zero values are skipped)
        let mut cursor150 = storage.storage_hashed_cursor(hashed_address, 150)?;
        let result150 = cursor150.seek(storage_key)?;
        assert!(result150.is_none(), "Zero values should be skipped/deleted");

        Ok(())
    }

    /// Test that zero values are skipped during iteration
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_storage_cursor_skips_zero_values<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let hashed_address = B256::repeat_byte(0x01);

        // Create a mix of non-zero and zero value storage slots
        let storage_slots = vec![
            (B256::repeat_byte(0x10), U256::from(100)), // Non-zero
            (B256::repeat_byte(0x20), U256::ZERO),      // Zero value - should be skipped
            (B256::repeat_byte(0x30), U256::from(300)), // Non-zero
            (B256::repeat_byte(0x40), U256::ZERO),      // Zero value - should be skipped
            (B256::repeat_byte(0x50), U256::from(500)), // Non-zero
        ];

        // Store all slots
        storage.store_hashed_storages(hashed_address, storage_slots.clone(), 50).await?;

        // Create cursor and iterate through all entries
        let mut cursor = storage.storage_hashed_cursor(hashed_address, 100)?;
        let mut found_slots = Vec::new();
        while let Some((key, value)) = cursor.next()? {
            found_slots.push((key, value));
        }

        // Should only find 3 non-zero values
        assert_eq!(found_slots.len(), 3, "Zero values should be skipped during iteration");

        // Verify the non-zero values are the ones we stored
        assert_eq!(found_slots[0], (B256::repeat_byte(0x10), U256::from(100)));
        assert_eq!(found_slots[1], (B256::repeat_byte(0x30), U256::from(300)));
        assert_eq!(found_slots[2], (B256::repeat_byte(0x50), U256::from(500)));

        // Verify seeking to a zero-value slot returns None or skips to next non-zero
        let mut seek_cursor = storage.storage_hashed_cursor(hashed_address, 100)?;
        let seek_result = seek_cursor.seek(B256::repeat_byte(0x20))?;

        // Should either return None or skip to the next non-zero value (0x30)
        if let Some((key, value)) = seek_result {
            assert_eq!(
                key,
                B256::repeat_byte(0x30),
                "Should skip zero value and find next non-zero"
            );
            assert_eq!(value, U256::from(300));
        }

        Ok(())
    }

    /// Test empty cursors
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_empty_cursors<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        // Test empty account cursor
        let mut account_cursor = storage.account_hashed_cursor(100)?;
        assert!(account_cursor.seek(B256::repeat_byte(0x01))?.is_none());
        assert!(account_cursor.next()?.is_none());

        // Test empty storage cursor
        let mut storage_cursor = storage.storage_hashed_cursor(B256::repeat_byte(0x01), 100)?;
        assert!(storage_cursor.seek(B256::repeat_byte(0x10))?.is_none());
        assert!(storage_cursor.next()?.is_none());

        Ok(())
    }

    /// Test cursor boundary conditions
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_cursor_boundary_conditions<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        let account_key = B256::repeat_byte(0x80); // Middle value
        let account = create_test_account();

        storage.store_hashed_accounts(vec![(account_key, Some(account))], 50).await?;

        let mut cursor = storage.account_hashed_cursor(100)?;

        // Seek to minimum key should find our account
        let result = cursor.seek(B256::ZERO)?.unwrap();
        assert_eq!(result.0, account_key);

        // Seek to maximum key should find nothing
        assert!(cursor.seek(B256::repeat_byte(0xFF))?.is_none());

        // Seek to key just before our account should find our account
        let just_before = B256::repeat_byte(0x7F);
        let result = cursor.seek(just_before)?.unwrap();
        assert_eq!(result.0, account_key);

        Ok(())
    }

    /// Test large batch operations
    #[test_case(InMemoryExternalStorage::new(); "InMemory")]
    #[tokio::test]
    async fn test_large_batch_operations<S: ExternalStorage>(
        storage: S,
    ) -> Result<(), ExternalStorageError> {
        // Create large batch of accounts
        let mut accounts = Vec::new();
        for i in 0..100 {
            let key = B256::from([i as u8; 32]);
            let account = create_test_account_with_values(i, i * 1000, (i + 1) as u8);
            accounts.push((key, Some(account)));
        }

        // Store in batch
        storage.store_hashed_accounts(accounts.clone(), 50).await?;

        // Verify all accounts can be retrieved
        let mut cursor = storage.account_hashed_cursor(100)?;
        let mut found_count = 0;
        while cursor.next()?.is_some() {
            found_count += 1;
        }
        assert_eq!(found_count, 100);

        // Test specific account retrieval
        let test_key = B256::from([42u8; 32]);
        let result = cursor.seek(test_key)?.unwrap();
        assert_eq!(result.0, test_key);
        assert_eq!(result.1.nonce, 42);

        Ok(())
    }
}
