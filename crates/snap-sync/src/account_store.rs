//! Persists account ranges, their storage and code, and the coverage cursor in one transaction.
//!
//! Each range replaces its key interval, and ranges commit in key order.

use crate::{SnapAttemptStore, SnapSyncError, SnapWrite};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, KECCAK256_EMPTY,
};
use reth_db_api::{
    cursor::DbCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable,
};
use reth_downloaders::snap::VerifiedAccountRange;
use reth_primitives_traits::Account;
use reth_storage_api::{DBProvider, MetadataProvider, MetadataWriter, SnapAttemptId, StateWriter};
use reth_storage_errors::provider::ProviderError;
use reth_trie_common::{
    root::storage_root_unsorted, HashedPostState, HashedStorage, EMPTY_ROOT_HASH,
};
use revm::{bytecode::Bytecode, database::states::StateChangeset};
use serde::{Deserialize, Serialize};
use std::ops::Bound;

/// Persistence for the account ranges an attempt downloads.
///
/// Blanket-implemented over the node's writers, so accounts, their dependencies and the coverage
/// join the caller's transaction and commit together or not at all.
pub trait SnapAccountStore {
    /// Returns the coverage recorded for the attempt `write` belongs to, recording that no
    /// account has been downloaded yet when there is none.
    fn start_account_coverage(&self, write: SnapWrite) -> Result<AccountCoverage, SnapSyncError>
    where
        Self: MetadataWriter;

    /// Returns the coverage recorded for the attempt `write` belongs to, if any.
    fn account_coverage(&self, write: SnapWrite) -> Result<Option<AccountCoverage>, SnapSyncError>;

    /// Persists `range` with its storage and code, replacing its key interval.
    ///
    /// Storage must match each account's root, and code must be supplied or already stored.
    fn commit_account_range(
        &self,
        write: SnapWrite,
        range: &VerifiedAccountRange,
        storages: B256Map<HashedStorage>,
        bytecodes: Vec<(B256, Bytecode)>,
    ) -> Result<AccountCoverage, SnapSyncError>
    where
        Self: MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>;
}

/// How far the account key space has been downloaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountCoverage {
    // Next key to request, or none once the trie is exhausted.
    next: Option<B256>,
}

impl AccountCoverage {
    /// Nothing downloaded yet.
    pub const START: Self = Self { next: Some(B256::ZERO) };

    /// Every account downloaded.
    pub const COMPLETE: Self = Self { next: None };

    /// Key the next range is requested from, or `None` once every account is downloaded.
    pub const fn next(&self) -> Option<B256> {
        self.next
    }

    /// Returns whether every account is downloaded.
    pub const fn is_complete(&self) -> bool {
        self.next.is_none()
    }
}

// Metadata key of the coverage record.
const COVERAGE_KEY: &str = "snap_account_coverage";

// Encoding version of the coverage record this build writes.
const COVERAGE_VERSION: u32 = 1;

// The coverage record as persisted, tied to the attempt that recorded it.
#[derive(Serialize, Deserialize)]
struct StoredCoverage {
    version: u32,
    attempt: SnapAttemptId,
    coverage: AccountCoverage,
}

impl StoredCoverage {
    fn encode(attempt: SnapAttemptId, coverage: AccountCoverage) -> Result<Vec<u8>, SnapSyncError> {
        let stored = Self { version: COVERAGE_VERSION, attempt, coverage };
        Ok(serde_json::to_vec(&stored).map_err(ProviderError::other)?)
    }

    // Checks the version first, so a record from another build is reported rather than misread.
    fn decode(bytes: &[u8]) -> Result<Self, SnapSyncError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(ProviderError::other)?;
        let version = value.get("version").and_then(serde_json::Value::as_u64);
        if version != Some(COVERAGE_VERSION as u64) {
            return Err(SnapSyncError::UnsupportedCoverage { version })
        }
        Ok(serde_json::from_value(value).map_err(ProviderError::other)?)
    }
}

impl<T: MetadataProvider> SnapAccountStore for T {
    fn start_account_coverage(&self, write: SnapWrite) -> Result<AccountCoverage, SnapSyncError>
    where
        Self: MetadataWriter,
    {
        if let Some(coverage) = self.account_coverage(write)? {
            return Ok(coverage)
        }
        let start = AccountCoverage::START;
        self.write_metadata(COVERAGE_KEY, StoredCoverage::encode(write.attempt(), start)?)?;
        Ok(start)
    }

    fn account_coverage(&self, write: SnapWrite) -> Result<Option<AccountCoverage>, SnapSyncError> {
        self.authorize_snap_write(write)?;
        let Some(bytes) = self.get_metadata(COVERAGE_KEY)? else { return Ok(None) };
        let stored = StoredCoverage::decode(&bytes)?;
        Ok((stored.attempt == write.attempt()).then_some(stored.coverage))
    }

    fn commit_account_range(
        &self,
        write: SnapWrite,
        range: &VerifiedAccountRange,
        storages: B256Map<HashedStorage>,
        bytecodes: Vec<(B256, Bytecode)>,
    ) -> Result<AccountCoverage, SnapSyncError>
    where
        Self: MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>,
    {
        let attempt = self.authorize_snap_write(write)?;
        if range.state_root() != attempt.state_root() {
            return Err(SnapSyncError::RootMismatch {
                expected: attempt.state_root(),
                got: range.state_root(),
            })
        }
        let coverage = self.account_coverage(write)?.ok_or(SnapSyncError::NoCoverage)?;
        let origin = range.origin();
        if coverage.next != Some(origin) {
            return Err(SnapSyncError::OutOfOrderRange { expected: coverage.next, got: origin })
        }
        if range.next().is_some_and(|next| next <= origin) {
            return Err(SnapSyncError::NoProgress { origin })
        }

        // Code known to be available: supplied and hashed, or already in the table.
        let mut available = B256Set::default();
        for (hash, code) in &bytecodes {
            let got = code.hash_slow();
            if got != *hash {
                return Err(SnapSyncError::CodeMismatch { expected: *hash, got })
            }
            available.insert(*hash);
        }
        let mut contracts = B256Set::default();
        for (hash, account) in range.accounts() {
            if account.storage_root != EMPTY_ROOT_HASH {
                let storage =
                    storages.get(hash).ok_or(SnapSyncError::MissingStorage { account: *hash })?;
                let got = storage_root_unsorted(
                    storage.storage.iter().filter(|(_, v)| !v.is_zero()).map(|(k, v)| (*k, *v)),
                );
                if got != account.storage_root {
                    return Err(SnapSyncError::StorageRootMismatch {
                        account: *hash,
                        expected: account.storage_root,
                        got,
                    })
                }
                contracts.insert(*hash);
            }
            if account.code_hash != KECCAK256_EMPTY && !available.contains(&account.code_hash) {
                // Only presence matters, so the code is not decoded.
                let key = RawKey::new(account.code_hash);
                if self.tx_ref().get::<RawTable<tables::Bytecodes>>(key)?.is_none() {
                    return Err(SnapSyncError::MissingCode { hash: account.code_hash })
                }
                available.insert(account.code_hash);
            }
        }
        if let Some(account) = storages.keys().find(|hash| !contracts.contains(*hash)) {
            return Err(SnapSyncError::UnexpectedStorage { account: *account })
        }

        // The range proves its interval holds only its accounts, so drop what an earlier
        // attempt left there.
        let interval =
            (Bound::Included(origin), range.next().map_or(Bound::Unbounded, Bound::Excluded));
        let mut accounts = self.tx_ref().cursor_write::<tables::HashedAccounts>()?;
        let mut walker = accounts.walk_range(interval)?;
        while walker.next().transpose()?.is_some() {
            walker.delete_current()?;
        }
        let mut slots = self.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        let mut walker = slots.walk_range(interval)?;
        while walker.next().transpose()?.is_some() {
            walker.delete_current()?;
        }

        let state = HashedPostState::default()
            .with_accounts(
                range
                    .accounts()
                    .iter()
                    .map(|(hash, account)| (*hash, Some(Account::from(*account)))),
            )
            .with_storages(storages);
        self.write_hashed_state(&state.into_sorted())?;
        self.write_state_changes(StateChangeset { contracts: bytecodes, ..Default::default() })?;

        let coverage = AccountCoverage { next: range.next() };
        self.write_metadata(COVERAGE_KEY, StoredCoverage::encode(write.attempt(), coverage)?)?;
        Ok(coverage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{account, generation, hashed_factory, key, state_root, verified_range},
        SnapGeneration,
    };
    use alloy_primitives::{Bytes, U256};
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, DatabaseProviderFactory, ProviderFactory,
    };
    use reth_trie_common::TrieAccount;

    const FAR: B256 = B256::repeat_byte(0xaa);
    const SLOT: B256 = B256::repeat_byte(0x55);

    fn code() -> Bytecode {
        Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]))
    }

    // Storage holding one slot, with the root the contract fixture commits to.
    fn storage() -> (B256, HashedStorage) {
        let storage = HashedStorage::from_iter([(SLOT, U256::from(7))]);
        (
            storage_root_unsorted(storage.storage.iter().map(|(slot, value)| (*slot, *value))),
            storage,
        )
    }

    // A plain account, a contract with storage and code, and a far account.
    fn accounts() -> Vec<(B256, TrieAccount)> {
        let mut contract = account(2);
        contract.storage_root = storage().0;
        contract.code_hash = code().hash_slow();
        vec![(key(1), account(1)), (key(2), contract), (FAR, account(3))]
    }

    // Storage and code that satisfy the contract in `accounts`.
    fn dependencies() -> (B256Map<HashedStorage>, Vec<(B256, Bytecode)>) {
        (B256Map::from_iter([(key(2), storage().1)]), vec![(code().hash_slow(), code())])
    }

    // An attempt anchored to the trie's root, with its coverage recorded.
    fn started(
        accounts: &[(B256, TrieAccount)],
    ) -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite, SnapGeneration) {
        let factory = hashed_factory();
        let generation = generation(1, state_root(accounts));
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider.commit().unwrap();
        (factory, write, generation)
    }

    fn stored_accounts(provider: &impl DBProvider) -> Vec<B256> {
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        cursor.walk(None).unwrap().map(|entry| entry.unwrap().0).collect()
    }

    fn stored(provider: &impl DBProvider) -> (Vec<B256>, bool, bool) {
        let tx = provider.tx_ref();
        (
            stored_accounts(provider),
            tx.get::<tables::HashedStorages>(key(2)).unwrap().is_some(),
            tx.get::<tables::Bytecodes>(code().hash_slow()).unwrap().is_some(),
        )
    }

    #[test]
    fn accounts_dependencies_and_coverage_commit_together() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, bytecodes) = dependencies();

        let provider = factory.database_provider_rw().unwrap();
        let coverage = provider.commit_account_range(write, &range, storages, bytecodes).unwrap();
        provider.commit().unwrap();

        assert!(coverage.is_complete());
        let provider = factory.database_provider_rw().unwrap();
        assert_eq!(stored(&provider), (vec![key(1), key(2), FAR], true, true));
        assert_eq!(provider.account_coverage(write).unwrap(), Some(coverage));
    }

    #[test]
    fn an_interrupted_commit_leaves_nothing_behind() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, bytecodes) = dependencies();

        let provider = factory.database_provider_rw().unwrap();
        provider.commit_account_range(write, &range, storages, bytecodes).unwrap();
        drop(provider);

        let provider = factory.database_provider_rw().unwrap();
        assert_eq!(stored(&provider), (Vec::new(), false, false));
        assert_eq!(provider.account_coverage(write).unwrap(), Some(AccountCoverage::START));
    }

    #[test]
    fn a_partial_range_moves_the_coverage_to_its_next_key() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        // Only the first account, with a proof placing the next one at key 2.
        let range = verified_range(&accounts, 0..1, B256::ZERO, &[key(1)]);

        let provider = factory.database_provider_rw().unwrap();
        let coverage =
            provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();

        assert_eq!(coverage.next(), Some(key(2)));
        assert_eq!(stored_accounts(&provider), [key(1)]);
    }

    #[test]
    fn a_proven_empty_tail_completes_the_coverage() {
        let accounts = vec![(key(1), account(1)), (key(2), account(2))];
        let (factory, write, _) = started(&accounts);
        let tail = verified_range(&accounts, 0..0, key(3), &[key(3)]);
        assert!(tail.accounts().is_empty());
        let provider = factory.database_provider_rw().unwrap();
        let coverage = AccountCoverage { next: Some(key(3)) };
        provider
            .write_metadata(
                COVERAGE_KEY,
                StoredCoverage::encode(write.attempt(), coverage).unwrap(),
            )
            .unwrap();

        let coverage =
            provider.commit_account_range(write, &tail, Default::default(), Vec::new()).unwrap();

        assert!(coverage.is_complete());
        assert!(stored_accounts(&provider).is_empty());
    }

    #[test]
    fn a_range_from_another_origin_changes_nothing() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let first = verified_range(&accounts, 0..1, B256::ZERO, &[key(1)]);
        provider.commit_account_range(write, &first, Default::default(), Vec::new()).unwrap();

        // The same range again, and one skipping ahead of the cursor.
        let duplicate =
            provider.commit_account_range(write, &first, Default::default(), Vec::new());
        let skipped = verified_range(&accounts, 2..3, FAR, &[FAR]);
        let ahead = provider.commit_account_range(write, &skipped, Default::default(), Vec::new());

        assert!(matches!(duplicate, Err(SnapSyncError::OutOfOrderRange { .. })));
        assert!(matches!(ahead, Err(SnapSyncError::OutOfOrderRange { .. })));
        assert_eq!(provider.account_coverage(write).unwrap().unwrap().next(), Some(key(2)));
        assert_eq!(stored_accounts(&provider), [key(1)]);
    }

    #[test]
    fn a_contract_without_its_storage_or_code_is_refused() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, bytecodes) = dependencies();
        let provider = factory.database_provider_rw().unwrap();

        let no_storage =
            provider.commit_account_range(write, &range, Default::default(), bytecodes.clone());
        assert!(
            matches!(no_storage, Err(SnapSyncError::MissingStorage { account }) if account == key(2))
        );

        let no_code = provider.commit_account_range(write, &range, storages, Vec::new());
        assert!(matches!(no_code, Err(SnapSyncError::MissingCode { .. })));

        let stray = B256Map::from_iter([(key(1), storage().1), (key(2), storage().1)]);
        let unexpected = provider.commit_account_range(write, &range, stray, bytecodes);
        assert!(
            matches!(unexpected, Err(SnapSyncError::UnexpectedStorage { account }) if account == key(1))
        );

        assert_eq!(stored(&provider), (Vec::new(), false, false));
        assert_eq!(provider.account_coverage(write).unwrap(), Some(AccountCoverage::START));
    }

    #[test]
    fn storage_and_code_are_checked_against_what_the_account_commits_to() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, bytecodes) = dependencies();
        let provider = factory.database_provider_rw().unwrap();

        // A slot short of the storage root, as a partial download would be.
        let partial = B256Map::from_iter([(key(2), HashedStorage::default())]);
        let short = provider.commit_account_range(write, &range, partial, bytecodes.clone());
        assert!(matches!(
            short,
            Err(SnapSyncError::StorageRootMismatch { account, expected, .. })
                if account == key(2) && expected == storage().0
        ));

        // Code filed under a hash it does not hash to.
        let other = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x01]));
        let relabelled = vec![(code().hash_slow(), other)];
        let wrong = provider.commit_account_range(write, &range, storages, relabelled);
        assert!(matches!(
            wrong,
            Err(SnapSyncError::CodeMismatch { expected, .. }) if expected == code().hash_slow()
        ));

        assert_eq!(stored(&provider), (Vec::new(), false, false));
        assert_eq!(provider.account_coverage(write).unwrap(), Some(AccountCoverage::START));
    }

    #[test]
    fn code_already_stored_need_not_be_supplied() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, _) = dependencies();
        let provider = factory.database_provider_rw().unwrap();
        provider
            .write_state_changes(StateChangeset {
                contracts: vec![(code().hash_slow(), code())],
                ..Default::default()
            })
            .unwrap();

        let coverage = provider.commit_account_range(write, &range, storages, Vec::new()).unwrap();

        assert!(coverage.is_complete());
    }

    #[test]
    fn starting_coverage_again_resumes_where_the_attempt_left_off() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..1, B256::ZERO, &[key(1)]);
        let provider = factory.database_provider_rw().unwrap();
        let committed =
            provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();

        assert_eq!(provider.start_account_coverage(write).unwrap(), committed);
        assert_eq!(provider.account_coverage(write).unwrap(), Some(committed));
    }

    #[test]
    fn a_write_from_a_replaced_attempt_changes_nothing() {
        let accounts = accounts();
        let (factory, replaced, generation) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, bytecodes) = dependencies();
        let provider = factory.database_provider_rw().unwrap();
        let current = provider.start_snap_attempt(generation).unwrap();

        let refused = provider.commit_account_range(replaced, &range, storages, bytecodes);

        assert!(matches!(refused, Err(SnapSyncError::StaleWrite { .. })));
        assert_eq!(stored(&provider), (Vec::new(), false, false));
        // The replaced attempt's coverage is not the new attempt's either.
        assert_eq!(provider.account_coverage(current).unwrap(), None);
        assert!(matches!(
            provider.account_coverage(replaced),
            Err(SnapSyncError::StaleWrite { .. })
        ));
    }

    #[test]
    fn a_new_attempt_replaces_what_an_earlier_one_left_in_the_interval() {
        let earlier = accounts();
        let (factory, write, _) = started(&earlier);
        let (storages, bytecodes) = dependencies();
        let provider = factory.database_provider_rw().unwrap();
        let range = verified_range(&earlier, 0..3, B256::ZERO, &[]);
        provider.commit_account_range(write, &range, storages, bytecodes).unwrap();
        provider.commit().unwrap();

        // At the new root the contract is gone and the first account changed.
        let current = vec![(key(1), account(9)), (FAR, account(3))];
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(2, state_root(&current))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(&current, 0..2, B256::ZERO, &[]);
        let coverage =
            provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();

        assert!(coverage.is_complete());
        assert_eq!(stored(&provider), (vec![key(1), FAR], false, true));
        let first = provider.tx_ref().get::<tables::HashedAccounts>(key(1)).unwrap().unwrap();
        assert_eq!(first.nonce, 9);
    }

    #[test]
    fn a_range_proved_against_another_root_is_refused() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let other = vec![(key(7), account(7))];
        let range = verified_range(&other, 0..1, B256::ZERO, &[]);

        let refused = provider.commit_account_range(write, &range, Default::default(), Vec::new());

        assert!(matches!(refused, Err(SnapSyncError::RootMismatch { .. })));
        assert!(stored_accounts(&provider).is_empty());
    }

    #[test]
    fn a_range_without_recorded_coverage_is_refused() {
        let accounts = accounts();
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1, state_root(&accounts))).unwrap();
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);

        assert!(matches!(
            provider.commit_account_range(write, &range, Default::default(), Vec::new()),
            Err(SnapSyncError::NoCoverage)
        ));
    }

    #[test]
    fn coverage_survives_reopening_the_database() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..1, B256::ZERO, &[key(1)]);
        let provider = factory.database_provider_rw().unwrap();
        let coverage =
            provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();

        let reopened = factory.database_provider_rw().unwrap();

        assert_eq!(reopened.account_coverage(write).unwrap(), Some(coverage));
    }

    #[test]
    fn a_record_this_build_cannot_read_is_reported() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);

        for record in [br#"{"version":999}"#.to_vec(), b"{}".to_vec()] {
            let provider = factory.database_provider_rw().unwrap();
            provider.write_metadata(COVERAGE_KEY, record).unwrap();

            assert!(matches!(
                provider.account_coverage(write),
                Err(SnapSyncError::UnsupportedCoverage { .. })
            ));
        }
    }
}
