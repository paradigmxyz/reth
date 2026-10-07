use crate::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_db_api::{cursor::DbDupCursorRO, table::DupSort, tables, transaction::DbTx};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    BytecodeReader, DBProvider, StateProofProvider, StorageRootProvider, StorageSettingsCache,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    hashed_cursor::{zero_destroyed_account_storage, HashedPostStateCursorFactory},
    proof::{Proof, StorageProof},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, DecodedMultiProofV2, ExecutionWitnessMode, HashedPostState, HashedStorage,
    KeccakKeyHasher, MultiProof, MultiProofTargets, MultiProofTargetsV2, StateRoot,
    StorageMultiProof, StorageRoot, TrieInput, TrieInputSorted,
};
use reth_trie_db::{DatabaseProof, DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot};
use std::{fmt, sync::Mutex};

type DbStateRoot<'a, TX, A> = StateRoot<
    reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
    reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
>;
type DbStorageRoot<'a, TX, A> = StorageRoot<
    reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
    reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
>;
type DbStorageProof<'a, TX, A> = StorageProof<
    'static,
    reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
    reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
>;
type DbProof<'a, TX, A> = Proof<
    reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
    reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
>;
/// State provider over latest state that takes tx reference.
///
/// Wraps a [`DBProvider`] to get access to database.
#[derive(Debug)]
pub struct LatestStateProviderRef<'b, Provider: DBProvider> {
    provider: &'b Provider,
    storage_cursors: StorageCursorCache<'b, Provider::Tx>,
}

impl<'b, Provider: DBProvider> LatestStateProviderRef<'b, Provider> {
    /// Create new state provider
    pub const fn new(provider: &'b Provider) -> Self {
        Self { provider, storage_cursors: StorageCursorCache::Owned(StorageCursors::new()) }
    }

    fn tx(&self) -> &Provider::Tx {
        self.provider.tx_ref()
    }

    fn hashed_storage_lookup(
        &self,
        hashed_address: B256,
        hashed_slot: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        self.with_storage_cursor::<tables::HashedStorages, _>(
            &self.storage_cursors.as_ref().hashed,
            |cursor| {
                Ok(cursor
                    .seek_by_key_subkey(hashed_address, hashed_slot)?
                    .filter(|e| e.key == hashed_slot)
                    .map(|e| e.value))
            },
        )
    }

    fn with_storage_cursor<T: DupSort, R>(
        &self,
        cached: &Mutex<Option<<Provider::Tx as DbTx>::DupCursor<T>>>,
        read: impl FnOnce(&mut <Provider::Tx as DbTx>::DupCursor<T>) -> ProviderResult<R>,
    ) -> ProviderResult<R> {
        // Each cursor belongs to this provider's transaction, never to a shared
        // cross-block cache. Concurrent readers and poisoned locks use a fresh
        // cursor instead of serializing independent storage queries.
        if let Ok(mut cursor) = cached.try_lock() {
            if cursor.is_none() {
                *cursor = Some(self.tx().cursor_dup_read::<T>()?);
            }
            read(cursor.as_mut().expect("cursor initialized"))
        } else {
            read(&mut self.tx().cursor_dup_read::<T>()?)
        }
    }
}

impl<Provider: DBProvider + StorageSettingsCache> AccountReader
    for LatestStateProviderRef<'_, Provider>
{
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if self.provider.cached_storage_settings().use_hashed_state() {
            let hashed_address = alloy_primitives::keccak256(address);
            self.tx()
                .get_by_encoded_key::<tables::HashedAccounts>(&hashed_address)
                .map_err(Into::into)
        } else {
            self.tx().get_by_encoded_key::<tables::PlainAccountState>(address).map_err(Into::into)
        }
    }
}

impl<Provider: DBProvider + BlockHashReader> BlockHashReader
    for LatestStateProviderRef<'_, Provider>
{
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.provider.canonical_hashes_range(start, end)
    }
}

impl<Provider: DBProvider + StorageSettingsCache> StateRootProvider
    for LatestStateProviderRef<'_, Provider>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let sorted = hashed_state.into_sorted();
            Ok(<DbStateRoot<'_, _, A> as DatabaseStateRoot<_>>::overlay_root(self.tx(), &sorted)?)
        })
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            Ok(<DbStateRoot<'_, _, A> as DatabaseStateRoot<_>>::overlay_root_from_nodes(
                self.tx(),
                TrieInputSorted::from_unsorted(input),
            )?)
        })
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let sorted = hashed_state.into_sorted();
            Ok(<DbStateRoot<'_, _, A> as DatabaseStateRoot<_>>::overlay_root_with_updates(
                self.tx(),
                &sorted,
            )?)
        })
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            Ok(
                <DbStateRoot<'_, _, A> as DatabaseStateRoot<_>>::overlay_root_from_nodes_with_updates(
                    self.tx(),
                    TrieInputSorted::from_unsorted(input),
                )?,
            )
        })
    }
}

impl<Provider: DBProvider + StorageSettingsCache> StorageRootProvider
    for LatestStateProviderRef<'_, Provider>
{
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let input = TrieInputSorted::from_state(
                HashedPostState::from_hashed_storage(
                    alloy_primitives::keccak256(address),
                    hashed_storage,
                )
                .into_sorted(),
            );
            <DbStorageRoot<'_, _, A>>::overlay_root(self.tx(), address, input)
                .map_err(|err| ProviderError::Database(err.into()))
        })
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            <DbStorageProof<'_, _, A>>::overlay_storage_proof(
                self.tx(),
                address,
                slot,
                hashed_storage,
            )
            .map_err(ProviderError::from)
        })
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let input = TrieInputSorted::from_state(
                HashedPostState::from_hashed_storage(
                    alloy_primitives::keccak256(address),
                    hashed_storage,
                )
                .into_sorted(),
            );
            <DbStorageProof<'_, _, A>>::overlay_storage_multiproof(self.tx(), address, slots, input)
                .map_err(ProviderError::from)
        })
    }
}

impl<Provider: DBProvider + StorageSettingsCache> StateProofProvider
    for LatestStateProviderRef<'_, Provider>
{
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(self.tx());
            proof.overlay_account_proof(input, address, slots).map_err(ProviderError::from)
        })
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(self.tx());
            proof.overlay_multiproof(input, targets).map_err(ProviderError::from)
        })
    }

    fn multiproof_v2(
        &self,
        input: TrieInput,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(self.tx());
            proof.overlay_multiproof_v2(input, targets).map_err(ProviderError::from)
        })
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        reth_trie_db::with_adapter!(self.provider, |A| {
            let nodes_sorted = input.nodes.into_sorted();
            let state_sorted = input.state.into_sorted();
            let witness = TrieWitness::new(
                InMemoryTrieCursorFactory::new(
                    reth_trie_db::DatabaseTrieCursorFactory::<_, A>::new(self.tx()),
                    &nodes_sorted,
                ),
                HashedPostStateCursorFactory::new(
                    reth_trie_db::DatabaseHashedCursorFactory::new(self.tx()),
                    &state_sorted,
                ),
            )
            .with_prefix_sets_mut(input.prefix_sets)
            .with_execution_witness_mode(mode);
            let witness =
                if mode.is_canonical() { witness } else { witness.always_include_root_node() };
            let mut values: Vec<_> = witness.compute(target)?.into_values().collect();
            if mode.is_canonical() {
                values.sort_unstable();
            }
            Ok(values)
        })
    }
}

impl<Provider: DBProvider> HashedPostStateProvider for LatestStateProviderRef<'_, Provider> {
    fn hashed_post_state(
        &self,
        bundle_state: &revm::database::BundleState,
    ) -> ProviderResult<HashedPostState> {
        let mut hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state());
        zero_destroyed_account_storage(
            &reth_trie_db::DatabaseHashedCursorFactory::new(self.tx()),
            bundle_state.state(),
            &mut hashed_state,
        )?;
        Ok(hashed_state)
    }
}

impl<Provider: DBProvider + BlockHashReader + StorageSettingsCache> StateProvider
    for LatestStateProviderRef<'_, Provider>
{
    /// Get storage by plain (unhashed) storage key slot.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        if self.provider.cached_storage_settings().use_hashed_state() {
            self.hashed_storage_lookup(
                alloy_primitives::keccak256(account),
                alloy_primitives::keccak256(storage_key),
            )
        } else {
            self.with_storage_cursor::<tables::PlainStorageState, _>(
                &self.storage_cursors.as_ref().plain,
                |cursor| {
                    Ok(cursor
                        .seek_by_key_subkey(account, storage_key)?
                        .filter(|entry| entry.key == storage_key)
                        .map(|entry| entry.value))
                },
            )
        }
    }
}

impl<Provider: DBProvider + BlockHashReader> BytecodeReader
    for LatestStateProviderRef<'_, Provider>
{
    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.tx().get_by_encoded_key::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

/// State provider for the latest state.
#[derive(Debug)]
pub struct LatestStateProvider<Provider: DBProvider> {
    // Release cursors before their provider; cursors may retain transaction handles.
    storage_cursors: StorageCursors<Provider::Tx>,
    provider: Provider,
}

impl<Provider: DBProvider> LatestStateProvider<Provider> {
    /// Create new state provider
    pub const fn new(db: Provider) -> Self {
        Self { storage_cursors: StorageCursors::new(), provider: db }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    const fn as_ref(&self) -> LatestStateProviderRef<'_, Provider> {
        LatestStateProviderRef {
            provider: &self.provider,
            storage_cursors: StorageCursorCache::Borrowed(&self.storage_cursors),
        }
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
reth_storage_api::macros::delegate_provider_impls!(LatestStateProvider<Provider> where [Provider: DBProvider + BlockHashReader + StorageSettingsCache]);

/// Storage cursors are scoped to one database transaction. Separate slots keep
/// storage-format changes from reusing a cursor for the wrong table.
struct StorageCursors<TX: DbTx> {
    plain: Mutex<Option<TX::DupCursor<tables::PlainStorageState>>>,
    hashed: Mutex<Option<TX::DupCursor<tables::HashedStorages>>>,
}

impl<TX: DbTx> StorageCursors<TX> {
    const fn new() -> Self {
        Self { plain: Mutex::new(None), hashed: Mutex::new(None) }
    }
}

impl<TX: DbTx> fmt::Debug for StorageCursors<TX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageCursors").finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum StorageCursorCache<'a, TX: DbTx> {
    Owned(StorageCursors<TX>),
    Borrowed(&'a StorageCursors<TX>),
}

impl<TX: DbTx> StorageCursorCache<'_, TX> {
    const fn as_ref(&self) -> &StorageCursors<TX> {
        match self {
            Self::Owned(cursors) => cursors,
            Self::Borrowed(cursors) => cursors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_test_provider_factory;
    use alloy_primitives::{address, b256, keccak256, U256};
    use reth_db_api::{
        models::StorageSettings,
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::StorageEntry;
    use reth_storage_api::StorageSettingsCache;

    const fn assert_state_provider<T: StateProvider>() {}
    #[expect(dead_code)]
    const fn assert_latest_state_provider<
        T: DBProvider + BlockHashReader + StorageSettingsCache,
    >() {
        assert_state_provider::<LatestStateProvider<T>>();
    }

    #[test]
    fn test_latest_storage_hashed_state() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: hashed_slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);

        assert_eq!(provider_ref.storage(address, slot).unwrap(), Some(U256::from(42)));

        let other_address = address!("0x0000000000000000000000000000000000000099");
        let other_slot =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000099");
        assert_eq!(provider_ref.storage(other_address, other_slot).unwrap(), None);

        let tx = factory.provider_rw().unwrap().into_tx();
        let plain_address = address!("0x0000000000000000000000000000000000000002");
        let plain_slot =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000002");
        tx.put::<tables::PlainStorageState>(
            plain_address,
            StorageEntry { key: plain_slot, value: U256::from(99) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);
        assert_eq!(provider_ref.storage(plain_address, plain_slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_hashed_state_returns_none_for_missing() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);
        assert_eq!(provider_ref.storage(address, slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_legacy() {
        let factory = create_test_provider_factory();
        assert!(!factory.provider().unwrap().cached_storage_settings().use_hashed_state());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000005");

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::PlainStorageState>(
            address,
            StorageEntry { key: slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);

        assert_eq!(provider_ref.storage(address, slot).unwrap(), Some(U256::from(42)));

        let other_slot =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000099");
        assert_eq!(provider_ref.storage(address, other_slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_legacy_does_not_read_hashed() {
        let factory = create_test_provider_factory();
        assert!(!factory.provider().unwrap().cached_storage_settings().use_hashed_state());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000005");
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: hashed_slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);
        assert_eq!(provider_ref.storage(address, slot).unwrap(), None);
    }

    fn write_cursor_fixture<TX: DbTxMut>(
        tx: &TX,
        hashed: bool,
        account: Address,
        slot: B256,
        value: u64,
        previous: Option<u64>,
    ) {
        if hashed {
            if let Some(previous) = previous {
                assert!(tx
                    .delete::<tables::HashedStorages>(
                        keccak256(account),
                        Some(StorageEntry { key: keccak256(slot), value: U256::from(previous) }),
                    )
                    .unwrap());
            }
            tx.put::<tables::HashedStorages>(
                keccak256(account),
                StorageEntry { key: keccak256(slot), value: U256::from(value) },
            )
            .unwrap();
        } else {
            if let Some(previous) = previous {
                assert!(tx
                    .delete::<tables::PlainStorageState>(
                        account,
                        Some(StorageEntry { key: slot, value: U256::from(previous) }),
                    )
                    .unwrap());
            }
            tx.put::<tables::PlainStorageState>(
                account,
                StorageEntry { key: slot, value: U256::from(value) },
            )
            .unwrap();
        }
    }

    #[test]
    fn retained_storage_cursors_preserve_reads_and_snapshot_isolation() {
        for hashed in [false, true] {
            let factory = create_test_provider_factory();
            if hashed {
                factory.set_storage_settings_cache(StorageSettings::v2());
            }
            let account = Address::from([1; 20]);
            let other = Address::from([2; 20]);
            let slot = B256::from([1; 32]);
            let missing = B256::from([2; 32]);
            let later = B256::from([3; 32]);
            let tx = factory.provider_rw().unwrap().into_tx();
            write_cursor_fixture(&tx, hashed, account, slot, 10, None);
            write_cursor_fixture(&tx, hashed, account, later, 30, None);
            write_cursor_fixture(&tx, hashed, other, slot, 40, None);
            tx.commit().unwrap();

            let old = LatestStateProvider::new(factory.provider().unwrap());
            let reference_db = factory.provider().unwrap();
            let borrowed = LatestStateProviderRef::new(&reference_db);
            let cases = [
                (account, slot, Some(U256::from(10))),
                (account, missing, None),
                (other, slot, Some(U256::from(40))),
                (Address::ZERO, slot, None),
                (account, later, Some(U256::from(30))),
            ];
            for _ in 0..16 {
                for (address, key, expected) in cases {
                    assert_eq!(old.storage(address, key).unwrap(), expected);
                    assert_eq!(borrowed.storage(address, key).unwrap(), expected);
                    // The original provider behavior opens a cursor for every query.
                    assert_eq!(
                        LatestStateProviderRef::new(&reference_db).storage(address, key).unwrap(),
                        expected
                    );
                }
            }
            if hashed {
                let held = old.storage_cursors.hashed.lock().unwrap();
                assert!(held.is_some());
                assert_eq!(old.storage(account, slot).unwrap(), Some(U256::from(10)));
            } else {
                let held = old.storage_cursors.plain.lock().unwrap();
                assert!(held.is_some());
                assert_eq!(old.storage(account, slot).unwrap(), Some(U256::from(10)));
            }

            let tx = factory.provider_rw().unwrap().into_tx();
            write_cursor_fixture(&tx, hashed, account, slot, 20, Some(10));
            write_cursor_fixture(&tx, hashed, account, missing, 22, None);
            tx.commit().unwrap();
            let new = LatestStateProvider::new(factory.provider().unwrap());
            for _ in 0..8 {
                assert_eq!(old.storage(account, slot).unwrap(), Some(U256::from(10)));
                assert_eq!(borrowed.storage(account, slot).unwrap(), Some(U256::from(10)));
                assert_eq!(old.storage(account, missing).unwrap(), None);
                assert_eq!(new.storage(account, slot).unwrap(), Some(U256::from(20)));
                assert_eq!(new.storage(account, missing).unwrap(), Some(U256::from(22)));
            }
        }
    }

    #[test]
    fn retained_storage_cursors_observe_writes_in_same_transaction() {
        for hashed in [false, true] {
            let factory = create_test_provider_factory();
            if hashed {
                factory.set_storage_settings_cache(StorageSettings::v2());
            }
            let writer = factory.provider_rw().unwrap();
            let view = LatestStateProviderRef::new(&*writer);
            let account = Address::from([1; 20]);
            let slot = B256::from([1; 32]);
            assert_eq!(view.storage(account, slot).unwrap(), None);
            write_cursor_fixture(writer.tx_ref(), hashed, account, slot, 10, None);
            assert_eq!(view.storage(account, slot).unwrap(), Some(U256::from(10)));
            write_cursor_fixture(writer.tx_ref(), hashed, account, slot, 20, Some(10));
            assert_eq!(view.storage(account, slot).unwrap(), Some(U256::from(20)));
        }
    }

    #[test]
    fn retained_storage_cursors_support_concurrent_readers() {
        for hashed in [false, true] {
            let factory = create_test_provider_factory();
            if hashed {
                factory.set_storage_settings_cache(StorageSettings::v2());
            }
            let tx = factory.provider_rw().unwrap().into_tx();
            for account in 1..=8 {
                for slot in 1..=64 {
                    write_cursor_fixture(
                        &tx,
                        hashed,
                        Address::from([account; 20]),
                        B256::from([slot; 32]),
                        u64::from(account) * 1000 + u64::from(slot),
                        None,
                    );
                }
            }
            tx.commit().unwrap();
            let provider = LatestStateProvider::new(factory.provider().unwrap());
            std::thread::scope(|scope| {
                for worker in 0..8 {
                    let provider = &provider;
                    scope.spawn(move || {
                        for query in 0..1024 {
                            let account = ((query * 7 + worker) % 10) as u8;
                            let slot = ((query * 13 + worker) % 66) as u8;
                            let expected = ((1..=8).contains(&account) && (1..=64).contains(&slot))
                                .then(|| U256::from(u64::from(account) * 1000 + u64::from(slot)));
                            assert_eq!(
                                provider
                                    .storage(Address::from([account; 20]), B256::from([slot; 32]))
                                    .unwrap(),
                                expected
                            );
                        }
                    });
                }
            });
        }
    }
}
