//! Trie integration tests.

#[cfg(test)]
mod tests {
    use alloy_primitives::{keccak256, Address, B256, U256};
    use reth_db::{test_utils::create_test_rw_db, transaction::DbTxMut, Database, HashedAccounts};
    use reth_primitives::Account;
    use reth_storage_errors::provider::ProviderError;
    use reth_trie::{
        prefix_set::TriePrefixSets,
        trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
        updates::TrieUpdates,
        walker::TrieWalker,
    };

    use reth_trie_db::{
        DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
    };
    use std::sync::Arc;

    #[test]
    fn test_trie_walker_with_real_db_populated_account() {
        let db = create_test_rw_db();

        // Create test data
        let address1 = Address::random();
        let account1 = Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None };

        let tx = db.db().tx_mut().unwrap();

        // Add account to the database
        tx.put::<HashedAccounts>(B256::from(keccak256(address1)), account1)
            .expect("Failed to insert account");

        // Commit the transaction
        tx.inner.commit().expect("Failed to commit transaction");

        // Create a read transaction for testing
        let tx = db.tx().expect("Failed to create transaction");

        // Test account trie walker
        let trie_updates = TrieUpdates::default();
        let trie_nodes_sorted = Arc::new(trie_updates.into_sorted());

        let db_cursor_factory = DatabaseTrieCursorFactory::new(&tx);
        // Verify that DatabaseTrieCursorFactory implements TrieCursorFactory
        let _: &dyn TrieCursorFactory<
            AccountTrieCursor = DatabaseAccountTrieCursor<_>,
            StorageTrieCursor = DatabaseStorageTrieCursor<_>,
        > = &db_cursor_factory;

        let trie_cursor_factory =
            InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(&tx), &trie_nodes_sorted);

        let prefix_sets = TriePrefixSets::default();
        let mut walker = TrieWalker::new(
            trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database).unwrap(),
            prefix_sets.account_prefix_set,
        );

        assert!(walker.key().is_some());
        let result = walker.advance().expect("Failed to advance walker");
        assert!(result.is_some());
    }

    /*
        #[test]
        fn test_trie_walker_with_real_db_populated_storage() {
            let db = create_test_rw_db();

            // Create test data
            let address1 = Address::random();
            let account1 = Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None };

            let tx = db.db().tx_mut().unwrap();

            // Add storage entry
            let storage_key = B256::random();
            let storage_value = StorageEntry { key: storage_key, value: U256::from(42) };
            tx.put::<HashedStorages>(B256::from(keccak256(address1)), storage_value)
                .expect("Failed to insert storage");

            // Commit the transaction
            tx.inner.commit().expect("Failed to commit transaction");

            // Create a read transaction for testing
            let tx = db.tx().expect("Failed to create transaction");

            let mut changes = PrefixSet::default();

            // Test storage trie walker
            let storage_cursor =
                tx.cursor_read::<HashedStorages>().expect("Failed to create storage cursor");
            let mut walker = TrieWalker::new(storage_cursor, changes);

            assert!(walker.key().is_some());
            let result = walker.advance().expect("Failed to advance walker");
            assert!(result.is_some());
    }
        */
}
