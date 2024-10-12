use std::collections::HashMap;

use crate::{
    providers::{state::macros::delegate_provider_impls, StaticFileProvider},
    AccountReader, BlockHashReader, StateProvider, StateRootProvider,
};
use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, StaticFileSegment, StorageKey, StorageValue,
    B256,
};
use reth_storage_api::StateProofProvider;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    prefix_set::TriePrefixSetsMut, proof::Proof, updates::TrieUpdates, witness::TrieWitness,
    AccountProof, HashedPostState, HashedStorage, StateRoot, StorageRoot,
};
use reth_trie_db::{DatabaseProof, DatabaseStateRoot, DatabaseStorageRoot, DatabaseTrieWitness};

/// State provider over latest state that takes tx reference.
#[derive(Debug)]
pub struct LatestStateProviderRef<'b, TX: DbTx> {
    /// database transaction
    tx: &'b TX,
    /// Static File provider
    static_file_provider: StaticFileProvider,
}

impl<'b, TX: DbTx> LatestStateProviderRef<'b, TX> {
    /// Create new state provider
    pub const fn new(tx: &'b TX, static_file_provider: StaticFileProvider) -> Self {
        Self { tx, static_file_provider }
    }
}

impl<'b, TX: DbTx> AccountReader for LatestStateProviderRef<'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.tx.get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<'b, TX: DbTx> BlockHashReader for LatestStateProviderRef<'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.block_hash(number),
            || Ok(self.tx.get::<tables::CanonicalHeaders>(number)?),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            start..end,
            |static_file, range, _| static_file.canonical_hashes_range(range.start, range.end),
            |range, _| {
                self.tx
                    .cursor_read::<tables::CanonicalHeaders>()
                    .map(|mut cursor| {
                        cursor
                            .walk_range(range)?
                            .map(|result| result.map(|(_, hash)| hash).map_err(Into::into))
                            .collect::<ProviderResult<Vec<_>>>()
                    })?
                    .map_err(Into::into)
            },
            |_| true,
        )
    }
}

impl<'b, TX: DbTx> StateRootProvider for LatestStateProviderRef<'b, TX> {
    fn hashed_state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        StateRoot::overlay_root(self.tx, hashed_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn hashed_state_root_from_nodes(
        &self,
        nodes: TrieUpdates,
        hashed_state: HashedPostState,
        prefix_sets: TriePrefixSetsMut,
    ) -> ProviderResult<B256> {
        StateRoot::overlay_root_from_nodes(self.tx, nodes, hashed_state, prefix_sets)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn hashed_state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_with_updates(self.tx, hashed_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn hashed_state_root_from_nodes_with_updates(
        &self,
        nodes: TrieUpdates,
        hashed_state: HashedPostState,
        prefix_sets: TriePrefixSetsMut,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_from_nodes_with_updates(self.tx, nodes, hashed_state, prefix_sets)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn hashed_storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        StorageRoot::overlay_root(self.tx, address, hashed_storage)
            .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<'b, TX: DbTx> StateProofProvider for LatestStateProviderRef<'b, TX> {
    fn hashed_proof(
        &self,
        hashed_state: HashedPostState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Proof::overlay_account_proof(self.tx, hashed_state, address, slots)
            .map_err(Into::<ProviderError>::into)
    }

    fn witness(
        &self,
        overlay: HashedPostState,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        TrieWitness::overlay_witness(self.tx, overlay, target).map_err(Into::<ProviderError>::into)
    }
}

impl<'b, TX: DbTx> StateProvider for LatestStateProviderRef<'b, TX> {
    /// Get storage.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let mut cursor = self.tx.cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? {
            if entry.key == storage_key {
                return Ok(Some(entry.value))
            }
        }
        Ok(None)
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        self.tx.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

/// State provider for the latest state.
#[derive(Debug)]
pub struct LatestStateProvider<TX: DbTx> {
    /// database transaction
    db: TX,
    /// Static File provider
    static_file_provider: StaticFileProvider,
}

impl<TX: DbTx> LatestStateProvider<TX> {
    /// Create new state provider
    pub const fn new(db: TX, static_file_provider: StaticFileProvider) -> Self {
        Self { db, static_file_provider }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> LatestStateProviderRef<'_, TX> {
        LatestStateProviderRef::new(&self.db, self.static_file_provider.clone())
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
delegate_provider_impls!(LatestStateProvider<TX> where [TX: DbTx]);

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_state_provider<T: StateProvider>() {}
    #[allow(dead_code)]
    const fn assert_latest_state_provider<T: DbTx>() {
        assert_state_provider::<LatestStateProvider<T>>();
    }
}
