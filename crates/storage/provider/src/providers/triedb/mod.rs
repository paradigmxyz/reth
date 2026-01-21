//! [`TrieDBProvider`] implementation
//!
//! This module provides a provider for TrieDB, a database designed for Ethereum state storage
//! using Merkle Patricia Tries.

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{Address, B256, U256};
use alloy_trie::EMPTY_ROOT_HASH;
use reth_db_api::DatabaseError;
use reth_primitives_traits::Account;
use reth_storage_errors::{
    db::DatabaseErrorInfo,
    provider::{ProviderError, ProviderResult},
};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};
use triedb::{
    account::Account as TrieDBAccount,
    path::AddressPath,
    transaction::{TransactionError, RW},
    Database as TrieDbDatabase,
};

#[cfg(feature = "triedb")]
use reth_storage_api::{PlainPostState, StateRootProvider};
#[cfg(feature = "triedb")]
use reth_trie::updates::TrieUpdates;
#[cfg(feature = "triedb")]
use reth_trie_common::{HashedPostState, TrieInput};
#[cfg(feature = "triedb")]
use triedb::{
    overlay::{OverlayStateMut, OverlayValue},
    path::StoragePath,
};

/// Builder for [`TrieDBProvider`].
pub struct TrieDBBuilder {
    path: PathBuf,
}

impl fmt::Debug for TrieDBBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrieDBBuilder").field("path", &self.path).finish()
    }
}

impl TrieDBBuilder {
    /// Creates a new builder.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self { path: path.as_ref().to_path_buf() }
    }

    /// Builds the [`TrieDBProvider`].
    ///
    /// Creates the database if it doesn't exist, or opens it if it does.
    pub fn build(self) -> ProviderResult<TrieDBProvider> {
        Self::ensure_parent_dir(&self.path)?;

        // Try to open existing database first
        let db = match TrieDbDatabase::open(&self.path) {
            Ok(db) => db,
            Err(_) => {
                // Database doesn't exist. If path exists and is a directory, remove it first
                // so create_new can succeed.
                if self.path.exists() && self.path.is_dir() {
                    std::fs::remove_dir(&self.path).map_err(|e| {
                        ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                            message: format!(
                                "Failed to remove empty directory at {:?}: {e}",
                                self.path
                            )
                            .into(),
                            code: -1,
                        }))
                    })?;
                }

                TrieDbDatabase::create_new(&self.path).map_err(|e| {
                    ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                        message: format!("Failed to create TrieDB at {:?}: {e:?}", self.path)
                            .into(),
                        code: -1,
                    }))
                })?
            }
        };

        Ok(TrieDBProvider(Arc::new(TrieDBProviderInner { db })))
    }

    /// Ensures the parent directory exists.
    fn ensure_parent_dir(path: &Path) -> ProviderResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                    message: format!("Failed to create parent directory: {e}").into(),
                    code: -1,
                }))
            })?;
        }
        Ok(())
    }
}

/// TrieDB provider for Ethereum state storage using Merkle Patricia Tries.
#[derive(Debug, Clone)]
pub struct TrieDBProvider(Arc<TrieDBProviderInner>);

/// Inner state for TrieDB provider.
struct TrieDBProviderInner {
    /// TrieDB database instance.
    db: TrieDbDatabase,
}

impl fmt::Debug for TrieDBProviderInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrieDBProviderInner").field("db", &"<TrieDbDatabase>").finish()
    }
}

impl TrieDBProvider {
    /// Creates a new TrieDB provider with default options.
    ///
    /// Equivalent to `TrieDBBuilder::new(path).build()`.
    pub fn new(path: impl AsRef<Path>) -> ProviderResult<Self> {
        TrieDBBuilder::new(path).build()
    }

    /// Creates a new TrieDB provider builder.
    pub fn builder(path: impl AsRef<Path>) -> TrieDBBuilder {
        TrieDBBuilder::new(path)
    }

    /// Creates a new read-write transaction.
    pub fn tx(&self) -> ProviderResult<TrieDBTx<'_>> {
        let inner = self.0.db.begin_rw().map_err(convert_transaction_error)?;
        Ok(TrieDBTx { inner, provider: self })
    }

    /// Creates a new batch for atomic writes.
    pub fn batch(&self) -> ProviderResult<TrieDBBatch<'_>> {
        let inner = self.0.db.begin_rw().map_err(convert_transaction_error)?;
        Ok(TrieDBBatch { inner, provider: self })
    }

    /// Gets an account by address.
    pub fn get_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        let mut tx = self.0.db.begin_ro().map_err(convert_transaction_error)?;
        let address_path = AddressPath::for_address(address);

        let trie_account_opt = tx.get_account(&address_path).map_err(convert_transaction_error)?;

        Ok(trie_account_opt.map(|trie_account| Account {
            nonce: trie_account.nonce,
            balance: trie_account.balance,
            bytecode_hash: if trie_account.code_hash == KECCAK_EMPTY {
                None
            } else {
                Some(trie_account.code_hash)
            },
        }))
    }

    /// Sets an account at the given address.
    ///
    /// This is a convenience method that opens a transaction, sets the account, and commits.
    /// For multiple operations, use [`Self::tx()`] or [`Self::batch()`] instead.
    pub fn set_account(
        &self,
        address: Address,
        account: Account,
        storage_root: Option<B256>,
    ) -> ProviderResult<()> {
        let mut tx = self.tx()?;
        tx.set_account(address, account, storage_root)?;
        tx.commit()
    }

    /// Writes a batch of operations atomically.
    pub fn write_batch<F>(&self, f: F) -> ProviderResult<()>
    where
        F: FnOnce(&mut TrieDBBatch<'_>) -> ProviderResult<()>,
    {
        let mut batch = self.batch()?;
        f(&mut batch)?;
        batch.commit()
    }

    /// Returns the current state root from TrieDB.
    pub fn state_root(&self) -> ProviderResult<B256> {
        Ok(self.0.db.state_root())
    }

    /// Returns a reference to the inner TrieDB database.
    ///
    /// This provides access to low-level TrieDB operations that aren't wrapped
    /// by the provider API.
    pub fn inner_db(&self) -> &TrieDbDatabase {
        &self.0.db
    }
}

/// Handle for building a batch of operations atomically.
#[must_use = "batch must be committed"]
pub struct TrieDBBatch<'a> {
    provider: &'a TrieDBProvider,
    inner: triedb::transaction::Transaction<&'a triedb::Database, RW>,
}

impl fmt::Debug for TrieDBBatch<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrieDBBatch")
            .field("provider", &self.provider)
            .field("transaction", &"<Transaction>")
            .finish()
    }
}

impl<'a> TrieDBBatch<'a> {
    /// Sets an account in the batch.
    pub fn set_account(
        &mut self,
        address: Address,
        account: Account,
        storage_root: Option<B256>,
    ) -> ProviderResult<()> {
        let address_path = AddressPath::for_address(address);
        let storage_root = storage_root.unwrap_or(EMPTY_ROOT_HASH);
        let trie_account = TrieDBAccount::new(
            account.nonce,
            account.balance,
            storage_root,
            account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        );

        self.inner
            .set_account(address_path, Some(trie_account))
            .map_err(convert_transaction_error)?;
        Ok(())
    }

    /// Deletes an account from the batch.
    pub fn delete_account(&mut self, address: Address) -> ProviderResult<()> {
        let address_path = AddressPath::for_address(address);
        self.inner.set_account(address_path, None).map_err(convert_transaction_error)?;
        Ok(())
    }

    /// Commits the batch to the database.
    pub fn commit(self) -> ProviderResult<()> {
        self.inner.commit().map_err(convert_transaction_error)
    }

    /// Returns a reference to the underlying TrieDB provider.
    pub const fn provider(&self) -> &TrieDBProvider {
        self.provider
    }
}

/// TrieDB transaction wrapper.
///
/// Supports:
/// - Read-your-writes: reads see uncommitted writes within the same transaction
/// - Atomic commit/rollback
pub struct TrieDBTx<'db> {
    inner: triedb::transaction::Transaction<&'db triedb::Database, RW>,
    provider: &'db TrieDBProvider,
}

impl fmt::Debug for TrieDBTx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrieDBTx").field("provider", &self.provider).finish_non_exhaustive()
    }
}

impl<'db> TrieDBTx<'db> {
    /// Gets an account by address. Sees uncommitted writes in this transaction.
    pub fn get_account(&mut self, address: Address) -> ProviderResult<Option<Account>> {
        let address_path = AddressPath::for_address(address);
        let trie_account_opt =
            self.inner.get_account(&address_path).map_err(convert_transaction_error)?;

        Ok(trie_account_opt.map(|trie_account| Account {
            nonce: trie_account.nonce,
            balance: trie_account.balance,
            bytecode_hash: if trie_account.code_hash == KECCAK_EMPTY {
                None
            } else {
                Some(trie_account.code_hash)
            },
        }))
    }

    /// Sets an account in the transaction.
    pub fn set_account(
        &mut self,
        address: Address,
        account: Account,
        storage_root: Option<B256>,
    ) -> ProviderResult<()> {
        let address_path = AddressPath::for_address(address);
        let storage_root = storage_root.unwrap_or(EMPTY_ROOT_HASH);
        let trie_account = TrieDBAccount::new(
            account.nonce,
            account.balance,
            storage_root,
            account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        );

        self.inner
            .set_account(address_path, Some(trie_account))
            .map_err(convert_transaction_error)?;
        Ok(())
    }

    /// Deletes an account from the transaction.
    pub fn delete_account(&mut self, address: Address) -> ProviderResult<()> {
        let address_path = AddressPath::for_address(address);
        self.inner.set_account(address_path, None).map_err(convert_transaction_error)?;
        Ok(())
    }

    /// Sets a storage slot for an account.
    pub fn set_storage(
        &mut self,
        address: Address,
        storage_key: alloy_primitives::StorageKey,
        storage_value: alloy_primitives::U256,
    ) -> ProviderResult<()> {
        use triedb::path::StoragePath;

        let address_path = AddressPath::for_address(address);
        let storage_path = StoragePath::for_address_path_and_slot(address_path, storage_key);

        let storage_value_triedb = if storage_value.is_zero() {
            None
        } else {
            Some(alloy_primitives::StorageValue::from_be_slice(
                storage_value.to_be_bytes::<32>().as_slice(),
            ))
        };

        self.inner
            .set_storage_slot(storage_path, storage_value_triedb)
            .map_err(convert_transaction_error)?;
        Ok(())
    }

    /// Commits the transaction, persisting all changes.
    pub fn commit(self) -> ProviderResult<()> {
        self.inner.commit().map_err(convert_transaction_error)
    }

    /// Rolls back the transaction, discarding all changes.
    pub fn rollback(self) -> ProviderResult<()> {
        // TrieDB transactions are automatically rolled back on drop if not committed
        drop(self.inner);
        Ok(())
    }
}

/// Converts TrieDB transaction error to provider error.
fn convert_transaction_error(err: TransactionError) -> ProviderError {
    ProviderError::Database(DatabaseError::Other(format!("TrieDB transaction error: {err:?}")))
}

/// Computes state root using TrieDB from a plain post state.
///
/// This function takes account and storage updates from the plain post state,
/// creates an overlay representing the changes, and computes the resulting state root
/// using TrieDB's overlay mechanism.
///
/// # Arguments
/// * `triedb_provider` - The TrieDB provider instance
/// * `plain_state` - The plain post state containing account and storage changes with unhashed
///   addresses
///
/// # Returns
/// A tuple of (state_root, trie_updates). Note that trie_updates will be empty as
/// TrieDB manages its own internal trie structure.
#[cfg(feature = "triedb")]
pub fn state_root_with_updates_triedb(
    triedb_provider: &TrieDBProvider,
    plain_state: PlainPostState,
) -> ProviderResult<(B256, TrieUpdates)> {
    tracing::debug!(target: "reth::triedb", "Computing state root with TrieDB");
    let start = std::time::Instant::now();

    // Create overlay for state changes
    let mut overlay_mut = OverlayStateMut::new();

    // Save length before consuming
    let accounts_len = plain_state.accounts.len();

    // Process account changes
    for (address, account_opt) in plain_state.accounts {
        let address_path = AddressPath::for_address(address);

        if let Some(account) = account_opt {
            let trie_account = TrieDBAccount::new(
                account.nonce,
                account.balance,
                EMPTY_ROOT_HASH, // Storage root will be computed from overlay
                account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            );
            overlay_mut
                .insert(address_path.clone().into(), Some(OverlayValue::Account(trie_account)));
        } else {
            // Account deleted
            overlay_mut.insert(address_path.clone().into(), None);
        }
    }

    tracing::debug!(target: "reth::triedb",
        "Processed {} account changes",
        accounts_len
    );

    // Process storage changes
    let mut total_storage = 0;
    for (address, storage) in plain_state.storages {
        let address_path = AddressPath::for_address(address);

        for (slot_b256, value) in storage {
            let storage_key =
                alloy_primitives::StorageKey::from(U256::from_be_slice(slot_b256.as_slice()));
            let storage_path =
                StoragePath::for_address_path_and_slot(address_path.clone(), storage_key);

            if value.is_zero() {
                // Storage slot deleted
                overlay_mut.insert(storage_path.into(), None);
            } else {
                overlay_mut.insert(
                    storage_path.into(),
                    Some(OverlayValue::Storage(alloy_primitives::StorageValue::from_be_slice(
                        value.to_be_bytes::<32>().as_slice(),
                    ))),
                );
            }
            total_storage += 1;
        }
    }

    tracing::debug!(target: "reth::triedb",
        "Processed {} storage changes",
        total_storage
    );

    let overlay_prep_time = start.elapsed();
    tracing::debug!(target: "reth::triedb",
        "Overlay preparation took {:?}",
        overlay_prep_time
    );

    // Freeze the overlay
    let freeze_start = std::time::Instant::now();
    let overlay = overlay_mut.freeze();
    tracing::debug!(target: "reth::triedb",
        "Overlay freeze took {:?}",
        freeze_start.elapsed()
    );

    // Compute root with overlay
    let compute_start = std::time::Instant::now();
    let tx = triedb_provider.0.db.begin_ro().map_err(|e| {
        ProviderError::TrieWitnessError(format!("Failed to begin TrieDB read transaction: {e:?}"))
    })?;

    let result = tx.compute_root_with_overlay(overlay).map_err(|e| {
        ProviderError::TrieWitnessError(format!("Failed to compute root with overlay: {e:?}"))
    })?;

    tracing::debug!(target: "reth::triedb",
        "Root computation took {:?}",
        compute_start.elapsed()
    );

    // Commit the read transaction
    let commit_start = std::time::Instant::now();
    tx.commit().map_err(|e| {
        ProviderError::TrieWitnessError(format!("Failed to commit TrieDB read transaction: {e:?}"))
    })?;

    tracing::debug!(target: "reth::triedb",
        "Transaction commit took {:?}",
        commit_start.elapsed()
    );

    tracing::debug!(target: "reth::triedb",
        state_root = ?result.root,
        total_time_ms = ?start.elapsed().as_millis(),
        "TrieDB state root computed successfully"
    );

    // Return state root with empty trie updates (TrieDB manages its own structure)
    Ok((result.root, TrieUpdates::default()))
}

impl StateRootProvider for TrieDBProvider {
    fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
        unimplemented!("Use state_root_with_updates_plain for TrieDB")
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        unimplemented!("Use state_root_with_updates_plain for TrieDB")
    }

    fn state_root_with_updates(
        &self,
        _hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!("Use state_root_with_updates_plain for TrieDB")
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!("Use state_root_with_updates_plain for TrieDB")
    }

    fn state_root_with_updates_plain(
        &self,
        plain_state: PlainPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        state_root_with_updates_triedb(self, plain_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use tempfile::TempDir;

    #[test]
    fn test_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("triedb");

        let provider = TrieDBBuilder::new(&db_path).build().unwrap();

        let address = Address::with_last_byte(1);
        let account = Account { nonce: 42, balance: U256::from(1000), bytecode_hash: None };

        provider.set_account(address, account, None).unwrap();

        let retrieved = provider.get_account(address).unwrap();
        assert!(retrieved.is_some());
        let acc = retrieved.unwrap();
        assert_eq!(acc.nonce, 42);
        assert_eq!(acc.balance, U256::from(1000));
        assert_eq!(acc.bytecode_hash, None);
    }

    #[test]
    fn test_multiple_providers() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("triedb");

        // Build with builder pattern
        let provider = TrieDBProvider::builder(&db_path).build().unwrap();

        let address = Address::with_last_byte(2);
        let account = Account { nonce: 1, balance: U256::from(5000), bytecode_hash: None };

        provider.set_account(address, account, None).unwrap();

        // Reopen with builder - should open existing database
        let provider2 = TrieDBProvider::builder(&db_path).build().unwrap();
        let retrieved = provider2.get_account(address).unwrap();
        assert!(retrieved.is_some());
        let acc = retrieved.unwrap();
        assert_eq!(acc.nonce, 1);
        assert_eq!(acc.balance, U256::from(5000));
        assert_eq!(acc.bytecode_hash, None);
    }

    #[test]
    fn test_batch_operations() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("triedb");
        let provider = TrieDBProvider::new(&db_path).unwrap();

        // Write multiple accounts in a batch
        provider
            .write_batch(|batch| {
                for i in 0..10u8 {
                    let address = Address::with_last_byte(i);
                    let account = Account {
                        nonce: i as u64,
                        balance: U256::from(i as u128 * 100),
                        bytecode_hash: None,
                    };
                    batch.set_account(address, account, None)?;
                }
                Ok(())
            })
            .unwrap();

        // Verify all accounts
        for i in 0..10u8 {
            let address = Address::with_last_byte(i);
            let account = provider.get_account(address).unwrap();
            assert!(account.is_some());
            assert_eq!(account.unwrap().nonce, i as u64);
        }
    }

    #[test]
    fn test_with_contract() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("triedb");
        let provider = TrieDBProvider::new(&db_path).unwrap();

        let address = Address::with_last_byte(5);
        let code_hash = B256::with_last_byte(0xFF);
        let account =
            Account { nonce: 1, balance: U256::from(5000), bytecode_hash: Some(code_hash) };

        provider.set_account(address, account, None).unwrap();

        let retrieved = provider.get_account(address).unwrap();
        assert!(retrieved.is_some());
        let acc = retrieved.unwrap();
        assert_eq!(acc.bytecode_hash, Some(code_hash));
    }

    #[test]
    fn test_nonexistent_account() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("triedb");
        let provider = TrieDBProvider::new(&db_path).unwrap();

        let nonexistent = Address::with_last_byte(99);
        let result = provider.get_account(nonexistent).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_account() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("triedb");
        let provider = TrieDBProvider::new(&db_path).unwrap();

        let address = Address::with_last_byte(10);
        let account = Account { nonce: 50, balance: U256::from(3000), bytecode_hash: None };

        // Set account
        provider.set_account(address, account, None).unwrap();
        assert!(provider.get_account(address).unwrap().is_some());

        // Delete via transaction
        let mut tx = provider.tx().unwrap();
        tx.delete_account(address).unwrap();
        tx.commit().unwrap();

        // Verify deletion
        assert!(provider.get_account(address).unwrap().is_none());
    }

    #[test]
    #[cfg(feature = "triedb")]
    fn test_triedb_matches_latest_state_provider_state_root() {
        use crate::test_utils::create_test_provider_factory;
        use reth_db::tables::PlainAccountState;
        use reth_db_api::transaction::DbTxMut;
        use reth_storage_api::{
            DBProvider, DatabaseProviderFactory, HashingWriter, PlainPostState, StateRootProvider,
        };
        use reth_trie::HashedPostState;
        use reth_trie_db::DatabaseStateRoot;

        // Create temp directory for TrieDB
        let temp_dir = TempDir::new().unwrap();
        let triedb_path = temp_dir.path().join("triedb");

        // Create TrieDB provider
        let triedb_provider = TrieDBProvider::new(&triedb_path).unwrap();

        // Create MDBX provider factory (creates its own temp database)
        let factory = create_test_provider_factory();

        // Create test data: some accounts with balances
        let accounts = vec![
            (
                Address::with_last_byte(1),
                Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None },
            ),
            (
                Address::with_last_byte(2),
                Account { nonce: 5, balance: U256::from(2000), bytecode_hash: None },
            ),
            (
                Address::with_last_byte(3),
                Account { nonce: 10, balance: U256::from(3000), bytecode_hash: None },
            ),
        ];

        // Populate TrieDB with initial state
        for (address, account) in &accounts {
            triedb_provider.set_account(*address, *account, None).unwrap();
        }

        // Write initial state to MDBX
        {
            let provider = factory.database_provider_rw().unwrap();

            // Write accounts to PlainAccountState
            for (address, account) in &accounts {
                provider.tx_ref().put::<PlainAccountState>(*address, *account).unwrap();
            }

            // Write hashed accounts for state root computation
            let accounts_for_hashing = accounts.iter().map(|(addr, acc)| (*addr, Some(*acc)));
            provider.insert_account_for_hashing(accounts_for_hashing).unwrap();

            provider.commit().unwrap();
        }

        // Now apply some state changes
        let mut changed_accounts = alloy_primitives::map::HashMap::default();

        // Update account 1: increase nonce and balance
        let updated_account1 = Account { nonce: 2, balance: U256::from(1500), bytecode_hash: None };
        changed_accounts.insert(Address::with_last_byte(1), Some(updated_account1));

        // Update account 2: change balance
        let updated_account2 = Account { nonce: 5, balance: U256::from(2500), bytecode_hash: None };
        changed_accounts.insert(Address::with_last_byte(2), Some(updated_account2));

        // Create PlainPostState for TrieDB
        let plain_post_state = PlainPostState {
            accounts: changed_accounts.clone(),
            storages: alloy_primitives::map::HashMap::default(),
        };

        // Create HashedPostState for MDBX (hash the addresses)
        let mut hashed_accounts = alloy_primitives::map::HashMap::default();
        for (addr, acc_opt) in &changed_accounts {
            let hashed_addr = alloy_primitives::keccak256(addr);
            hashed_accounts.insert(hashed_addr, *acc_opt);
        }

        let hashed_post_state = HashedPostState {
            accounts: hashed_accounts,
            storages: alloy_primitives::map::HashMap::default(),
        };

        // Compute state root via TrieDB
        let (triedb_state_root, _) =
            triedb_provider.state_root_with_updates_plain(plain_post_state).unwrap();

        // Compute state root via MDBX (LatestStateProvider)
        let db_provider = factory.database_provider_rw().unwrap();

        // Write unhashed account changes for hashing (insert_account_for_hashing hashes them
        // internally)
        let accounts_iter = changed_accounts.iter().map(|(addr, acc)| (*addr, *acc));
        db_provider.insert_account_for_hashing(accounts_iter).unwrap();

        use reth_trie::StateRoot;
        let mdbx_state_root =
            StateRoot::overlay_root(db_provider.tx_ref(), &hashed_post_state.into_sorted())
                .unwrap();

        // Assert state roots match
        assert_eq!(
            triedb_state_root, mdbx_state_root,
            "TrieDB and LatestStateProvider must compute the same state root.\n  TrieDB: {:?}\n  MDBX:   {:?}",
            triedb_state_root, mdbx_state_root
        );
    }
}
