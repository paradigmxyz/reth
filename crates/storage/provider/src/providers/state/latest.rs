use crate::{
    providers::{state::macros::delegate_provider_impls, StaticFileProvider},
    AccountReader, BlockHashReader, StateProvider, StateRootProvider,
};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Address, BlockNumber, Bytes, StorageKey, StorageValue, B256,
};
use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_primitives::{Account, Bytecode, StaticFileSegment};
use reth_storage_api::{StateProofProvider, StorageRootProvider};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    proof::{Proof, StorageProof},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, HashedPostState, HashedStorage, MultiProof, StateRoot, StorageRoot, TrieInput,
};
use reth_trie_db::{
    DatabaseProof, DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot,
    DatabaseTrieWitness,
};

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

impl<TX: DbTx> AccountReader for LatestStateProviderRef<'_, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.tx.get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<TX: DbTx> BlockHashReader for LatestStateProviderRef<'_, TX> {
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

impl<TX: DbTx> StateRootProvider for LatestStateProviderRef<'_, TX> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        StateRoot::overlay_root(self.tx, hashed_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        StateRoot::overlay_root_from_nodes(self.tx, input)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_with_updates(self.tx, hashed_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_from_nodes_with_updates(self.tx, input)
            .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<TX: DbTx> StorageRootProvider for LatestStateProviderRef<'_, TX> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        StorageRoot::overlay_root(self.tx, address, hashed_storage)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        StorageProof::overlay_storage_proof(self.tx, address, slot, hashed_storage)
            .map_err(Into::<ProviderError>::into)
    }
}

impl<TX: DbTx> StateProofProvider for LatestStateProviderRef<'_, TX> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Proof::overlay_account_proof(self.tx, input, address, slots)
            .map_err(Into::<ProviderError>::into)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        Proof::overlay_multiproof(self.tx, input, targets).map_err(Into::<ProviderError>::into)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        TrieWitness::overlay_witness(self.tx, input, target).map_err(Into::<ProviderError>::into)
    }
}

impl<TX: DbTx> StateProvider for LatestStateProviderRef<'_, TX> {
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
