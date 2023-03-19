use crate::{
    providers::state::macros::delegate_provider_impls, trie::DBTrieLoader, AccountProvider,
    BlockHashProvider, StateProvider,
};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{
    keccak256, Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, H256,
    KECCAK_EMPTY,
};
use std::marker::PhantomData;

/// State provider over latest state that takes tx reference.
pub struct LatestStateProviderRef<'a, 'b, TX: DbTx<'a>> {
    /// database transaction
    db: &'b TX,
    /// Phantom data over lifetime
    phantom: PhantomData<&'a TX>,
}

impl<'a, 'b, TX: DbTx<'a>> LatestStateProviderRef<'a, 'b, TX> {
    /// Create new state provider
    pub fn new(db: &'b TX) -> Self {
        Self { db, phantom: PhantomData {} }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountProvider for LatestStateProviderRef<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        self.db.get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> BlockHashProvider for LatestStateProviderRef<'a, 'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        self.db.get::<tables::CanonicalHeaders>(number).map_err(Into::into)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        let range = start..end;
        self.db
            .cursor_read::<tables::CanonicalHeaders>()
            .map(|mut cursor| {
                cursor
                    .walk_range(range)?
                    .map(|result| result.map(|(_, hash)| hash).map_err(Into::into))
                    .collect::<Result<Vec<_>>>()
            })?
            .map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for LatestStateProviderRef<'a, 'b, TX> {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        let mut cursor = self.db.cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? {
            if entry.key == storage_key {
                return Ok(Some(entry.value))
            }
        }
        Ok(None)
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        self.db.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }

    fn proof(
        &self,
        address: Address,
        keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        let hashed_address = keccak256(address);
        let loader = DBTrieLoader::new(self.db);
        let root = self
            .db
            .cursor_read::<tables::Headers>()?
            .last()?
            .ok_or(ProviderError::Header { number: 0 })?
            .1
            .state_root;

        let (account_proof, storage_root) = loader
            .generate_acount_proof(root, hashed_address)
            .map_err(|_| ProviderError::StateTrie)?;
        let account_proof = account_proof.into_iter().map(Bytes::from).collect();

        let storage_proof = if storage_root == KECCAK_EMPTY {
            // if there isn't storage, we return empty storage proofs
            (0..keys.len()).map(|_| Vec::new()).collect()
        } else {
            let hashed_keys: Vec<H256> = keys.iter().map(keccak256).collect();
            loader
                .generate_storage_proofs(storage_root, hashed_address, &hashed_keys)
                .map_err(|_| ProviderError::StateTrie)?
                .into_iter()
                .map(|v| v.into_iter().map(Bytes::from).collect())
                .collect()
        };

        Ok((account_proof, storage_root, storage_proof))
    }
}

/// State provider for the latest state.
pub struct LatestStateProvider<'a, TX: DbTx<'a>> {
    /// database transaction
    db: TX,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> LatestStateProvider<'a, TX> {
    /// Create new state provider
    pub fn new(db: TX) -> Self {
        Self { db, _phantom: PhantomData {} }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref<'b>(&'b self) -> LatestStateProviderRef<'a, 'b, TX> {
        LatestStateProviderRef::new(&self.db)
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
delegate_provider_impls!(LatestStateProvider<'a, TX> where [TX: DbTx<'a>]);

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_state_provider<T: StateProvider>() {}
    #[allow(unused)]
    fn assert_latest_state_provider<'txn, T: DbTx<'txn> + 'txn>() {
        assert_state_provider::<LatestStateProvider<'txn, T>>();
    }
}
