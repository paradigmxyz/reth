use crate::{
    providers::{state::macros::delegate_provider_impls, SnapshotProvider},
    AccountReader, BlockHashReader, BundleStateWithReceipts, StateProvider, StateRootProvider,
};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_primitives::{
    trie::AccountProof, Account, Address, BlockNumber, Bytecode, SnapshotSegment, StorageKey,
    StorageValue, B256,
};
use reth_trie::{proof::Proof, updates::TrieUpdates};
use std::sync::Arc;

/// State provider over latest state that takes tx reference.
#[derive(Debug)]
pub struct LatestStateProviderRef<'b, TX: DbTx> {
    /// database transaction
    db: &'b TX,
    /// Snapshot provider
    snapshot_provider: Arc<SnapshotProvider>,
}

impl<'b, TX: DbTx> LatestStateProviderRef<'b, TX> {
    /// Create new state provider
    pub fn new(db: &'b TX, snapshot_provider: Arc<SnapshotProvider>) -> Self {
        Self { db, snapshot_provider }
    }
}

impl<'b, TX: DbTx> AccountReader for LatestStateProviderRef<'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.db.get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<'b, TX: DbTx> BlockHashReader for LatestStateProviderRef<'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.snapshot_provider.get_with_snapshot_or_database(
            SnapshotSegment::Headers,
            number,
            |snapshot| snapshot.block_hash(number),
            || Ok(self.db.get::<tables::CanonicalHeaders>(number)?),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.snapshot_provider.get_range_with_snapshot_or_database(
            SnapshotSegment::Headers,
            start..end,
            |snapshot, range, _| snapshot.canonical_hashes_range(range.start, range.end),
            |range, _| {
                self.db
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
    fn state_root(&self, bundle_state: &BundleStateWithReceipts) -> ProviderResult<B256> {
        bundle_state
            .hash_state_slow()
            .state_root(self.db)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(
        &self,
        bundle_state: &BundleStateWithReceipts,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        bundle_state
            .hash_state_slow()
            .state_root_with_updates(self.db)
            .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<'b, TX: DbTx> StateProvider for LatestStateProviderRef<'b, TX> {
    /// Get storage.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let mut cursor = self.db.cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? {
            if entry.key == storage_key {
                return Ok(Some(entry.value))
            }
        }
        Ok(None)
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        self.db.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }

    fn proof(&self, address: Address, slots: &[B256]) -> ProviderResult<AccountProof> {
        Ok(Proof::new(self.db)
            .account_proof(address, slots)
            .map_err(Into::<reth_db::DatabaseError>::into)?)
    }
}

/// State provider for the latest state.
#[derive(Debug)]
pub struct LatestStateProvider<TX: DbTx> {
    /// database transaction
    db: TX,
    /// Snapshot provider
    snapshot_provider: Arc<SnapshotProvider>,
}

impl<TX: DbTx> LatestStateProvider<TX> {
    /// Create new state provider
    pub fn new(db: TX, snapshot_provider: Arc<SnapshotProvider>) -> Self {
        Self { db, snapshot_provider }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> LatestStateProviderRef<'_, TX> {
        LatestStateProviderRef::new(&self.db, self.snapshot_provider.clone())
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
delegate_provider_impls!(LatestStateProvider<TX> where [TX: DbTx]);

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_state_provider<T: StateProvider>() {}
    #[allow(dead_code)]
    fn assert_latest_state_provider<T: DbTx>() {
        assert_state_provider::<LatestStateProvider<T>>();
    }
}
