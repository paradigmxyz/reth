//! [`TrieDBProvider`] implementation
//!
//! This module provides a provider for TrieDB, a database designed for Ethereum state storage
//! using Merkle Patricia Tries.

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{Address, B256};
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
    transaction::{RW, TransactionError},
    Database as TrieDbDatabase,
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
                            message: format!("Failed to remove empty directory at {:?}: {e}", self.path).into(),
                            code: -1,
                        }))
                    })?;
                }

                TrieDbDatabase::create_new(&self.path).map_err(|e| {
                    ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                        message: format!("Failed to create TrieDB at {:?}: {e:?}", self.path).into(),
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
                    let account =
                        Account { nonce: i as u64, balance: U256::from(i as u128 * 100), bytecode_hash: None };
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
        let account = Account { nonce: 1, balance: U256::from(5000), bytecode_hash: Some(code_hash) };

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
}
