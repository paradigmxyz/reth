//! Stub implementation of `TrieDB` provider.
//!
//! This module provides placeholder types that allow the code to compile when the `triedb`
//! feature is not enabled. Operations will produce errors if actually attempted.

use alloy_primitives::Address;
use reth_primitives_traits::Account;
use reth_storage_errors::provider::{ProviderError::UnsupportedProvider, ProviderResult};
use std::path::Path;

/// A stub `TrieDB` provider.
#[derive(Debug, Clone)]
pub struct TrieDBProvider;

impl TrieDBProvider {
    /// Creates a new stub `TrieDB` provider.
    pub fn new(_path: impl AsRef<Path>) -> ProviderResult<Self> {
        Ok(Self)
    }

    /// Creates a new stub `TrieDB` provider builder.
    pub fn builder(path: impl AsRef<Path>) -> TrieDBBuilder {
        TrieDBBuilder::new(path)
    }

    /// Gets an account by address (stub implementation).
    pub const fn get_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
        Err(UnsupportedProvider)
    }

    /// Sets an account at the given address (stub implementation).
    pub const fn set_account(
        &self,
        _address: Address,
        _account: Account,
        _storage_root: Option<alloy_primitives::B256>,
    ) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Creates a new read-write transaction (stub implementation).
    pub const fn tx(&self) -> ProviderResult<TrieDBTx> {
        Err(UnsupportedProvider)
    }

    /// Creates a new batch for atomic writes (stub implementation).
    pub const fn batch(&self) -> ProviderResult<TrieDBBatch> {
        Err(UnsupportedProvider)
    }

    /// Writes a batch of operations atomically (stub implementation).
    pub fn write_batch<F>(&self, _f: F) -> ProviderResult<()>
    where
        F: FnOnce(&mut TrieDBBatch) -> ProviderResult<()>,
    {
        Err(UnsupportedProvider)
    }
}

/// A stub batch writer for `TrieDB`.
#[derive(Debug)]
pub struct TrieDBBatch;

impl TrieDBBatch {
    /// Sets an account in the batch (stub implementation).
    pub const fn set_account(
        &mut self,
        _address: Address,
        _account: Account,
        _storage_root: Option<alloy_primitives::B256>,
    ) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Deletes an account from the batch (stub implementation).
    pub const fn delete_account(&mut self, _address: Address) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Commits the batch to the database (stub implementation).
    pub const fn commit(self) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Returns a reference to the underlying TrieDB provider (stub implementation).
    pub const fn provider(&self) -> &TrieDBProvider {
        &TrieDBProvider
    }
}

/// A stub builder for `TrieDB`.
#[derive(Debug)]
pub struct TrieDBBuilder;

impl TrieDBBuilder {
    /// Creates a new stub builder.
    pub fn new<P: AsRef<Path>>(_path: P) -> Self {
        Self
    }

    /// Build the `TrieDB` provider (stub implementation).
    pub const fn build(self) -> ProviderResult<TrieDBProvider> {
        Ok(TrieDBProvider)
    }
}

/// A stub transaction for `TrieDB`.
#[derive(Debug)]
pub struct TrieDBTx;

impl TrieDBTx {
    /// Gets an account by address (stub implementation).
    pub const fn get_account(&mut self, _address: Address) -> ProviderResult<Option<Account>> {
        Err(UnsupportedProvider)
    }

    /// Sets an account in the transaction (stub implementation).
    pub const fn set_account(
        &mut self,
        _address: Address,
        _account: Account,
        _storage_root: Option<alloy_primitives::B256>,
    ) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Deletes an account from the transaction (stub implementation).
    pub const fn delete_account(&mut self, _address: Address) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Commits the transaction (stub implementation).
    pub const fn commit(self) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Rolls back the transaction (stub implementation).
    pub const fn rollback(self) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }
}
