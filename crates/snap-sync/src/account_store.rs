//! Persists downloaded account ranges with how far they take the coverage.
//!
//! Accounts land in the hashed state tables with their storage and code, through the writers
//! execution uses. The coverage cursor commits in the same transaction, under the write of the
//! attempt that owns it, so a response from a superseded attempt is refused before it changes
//! anything, and committed progress never depends on work still pending.

use crate::{SnapAttemptStore, SnapSyncError, SnapWrite};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, KECCAK256_EMPTY,
};
use reth_db_api::{tables, transaction::DbTx};
use reth_downloaders::snap::VerifiedAccountRange;
use reth_primitives_traits::Account;
use reth_storage_api::{DBProvider, MetadataProvider, MetadataWriter, SnapAttemptId, StateWriter};
use reth_storage_errors::provider::ProviderError;
use reth_trie_common::{HashedPostState, HashedStorage, EMPTY_ROOT_HASH};
use revm::{bytecode::Bytecode, database::states::StateChangeset};
use serde::{Deserialize, Serialize};

/// Persistence for the account ranges an attempt downloads.
///
/// Blanket-implemented over the node's writers, so accounts, their dependencies and the coverage
/// join the caller's transaction and commit together or not at all.
pub trait SnapAccountStore {
    /// Records that no account has been downloaded yet.
    fn start_account_coverage(&self, write: SnapWrite) -> Result<AccountCoverage, SnapSyncError>;

    /// Returns the coverage recorded for the attempt `write` belongs to, if any.
    fn account_coverage(&self, write: SnapWrite) -> Result<Option<AccountCoverage>, SnapSyncError>;

    /// Persists `range`, requested from `origin`, with the storage and code its accounts need.
    ///
    /// Every contract in the range must have its storage in `storages`, and every code hash must
    /// be in `bytecodes` or already stored. The coverage moves to the key after the range.
    fn commit_account_range(
        &self,
        write: SnapWrite,
        origin: B256,
        range: &VerifiedAccountRange,
        storages: B256Map<HashedStorage>,
        bytecodes: Vec<(B256, Bytecode)>,
    ) -> Result<AccountCoverage, SnapSyncError>;
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

impl<T> SnapAccountStore for T
where
    T: MetadataProvider + MetadataWriter + StateWriter + DBProvider,
{
    fn start_account_coverage(&self, write: SnapWrite) -> Result<AccountCoverage, SnapSyncError> {
        self.authorize_snap_write(write)?;
        write_coverage(self, write.attempt(), AccountCoverage::START)?;
        Ok(AccountCoverage::START)
    }

    fn account_coverage(&self, write: SnapWrite) -> Result<Option<AccountCoverage>, SnapSyncError> {
        self.authorize_snap_write(write)?;
        let Some(bytes) = self.get_metadata(COVERAGE_KEY)? else { return Ok(None) };

        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(ProviderError::other)?;
        let version = value.get("version").and_then(serde_json::Value::as_u64);
        if version != Some(COVERAGE_VERSION as u64) {
            return Err(SnapSyncError::UnsupportedCoverage { version })
        }
        let stored: StoredCoverage =
            serde_json::from_slice(&bytes).map_err(ProviderError::other)?;
        Ok((stored.attempt == write.attempt()).then_some(stored.coverage))
    }

    fn commit_account_range(
        &self,
        write: SnapWrite,
        origin: B256,
        range: &VerifiedAccountRange,
        storages: B256Map<HashedStorage>,
        bytecodes: Vec<(B256, Bytecode)>,
    ) -> Result<AccountCoverage, SnapSyncError> {
        let attempt = self.authorize_snap_write(write)?;
        if range.state_root() != attempt.state_root() {
            return Err(SnapSyncError::RootMismatch {
                expected: attempt.state_root(),
                got: range.state_root(),
            })
        }
        let coverage = self.account_coverage(write)?.ok_or(SnapSyncError::NoCoverage)?;
        if coverage.next != Some(origin) {
            return Err(SnapSyncError::OutOfOrderRange { expected: coverage.next, got: origin })
        }
        if range.next().is_some_and(|next| next <= origin) {
            return Err(SnapSyncError::NoProgress { origin })
        }

        let supplied: B256Set = bytecodes.iter().map(|(hash, _)| *hash).collect();
        let mut contracts = B256Set::default();
        for (hash, account) in range.accounts() {
            if account.storage_root != EMPTY_ROOT_HASH {
                if !storages.contains_key(hash) {
                    return Err(SnapSyncError::MissingStorage { account: *hash })
                }
                contracts.insert(*hash);
            }
            if account.code_hash != KECCAK256_EMPTY &&
                !supplied.contains(&account.code_hash) &&
                self.tx_ref().get::<tables::Bytecodes>(account.code_hash)?.is_none()
            {
                return Err(SnapSyncError::MissingCode { hash: account.code_hash })
            }
        }
        if let Some(account) = storages.keys().find(|hash| !contracts.contains(*hash)) {
            return Err(SnapSyncError::UnexpectedStorage { account: *account })
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
        write_coverage(self, write.attempt(), coverage)?;
        Ok(coverage)
    }
}

fn write_coverage(
    store: &impl MetadataWriter,
    attempt: SnapAttemptId,
    coverage: AccountCoverage,
) -> Result<(), SnapSyncError> {
    let stored = StoredCoverage { version: COVERAGE_VERSION, attempt, coverage };
    store
        .write_metadata(COVERAGE_KEY, serde_json::to_vec(&stored).map_err(ProviderError::other)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{account, generation, hashed_factory, key, state_root, verified_range},
        SnapGeneration,
    };
    use alloy_primitives::{Bytes, U256};
    use reth_db_api::cursor::DbCursorRO;
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, DatabaseProviderFactory, ProviderFactory,
    };
    use reth_trie_common::{root::storage_root_unsorted, TrieAccount};

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
        let coverage =
            provider.commit_account_range(write, B256::ZERO, &range, storages, bytecodes).unwrap();
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
        provider.commit_account_range(write, B256::ZERO, &range, storages, bytecodes).unwrap();
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
        let coverage = provider
            .commit_account_range(write, B256::ZERO, &range, Default::default(), Vec::new())
            .unwrap();

        assert_eq!(coverage.next(), Some(key(2)));
        assert_eq!(stored_accounts(&provider), [key(1)]);
    }

    #[test]
    fn a_proven_empty_tail_completes_the_coverage() {
        let accounts = vec![(key(1), account(1)), (key(2), account(2))];
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..0, key(3), &[key(3)]);
        assert!(range.accounts().is_empty());
        let provider = factory.database_provider_rw().unwrap();
        provider
            .write_metadata(
                COVERAGE_KEY,
                serde_json::to_vec(&StoredCoverage {
                    version: COVERAGE_VERSION,
                    attempt: write.attempt(),
                    coverage: AccountCoverage { next: Some(key(3)) },
                })
                .unwrap(),
            )
            .unwrap();

        let coverage = provider
            .commit_account_range(write, key(3), &range, Default::default(), Vec::new())
            .unwrap();

        assert!(coverage.is_complete());
        assert!(stored_accounts(&provider).is_empty());
    }

    #[test]
    fn a_range_from_another_origin_changes_nothing() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let first = verified_range(&accounts, 0..1, B256::ZERO, &[key(1)]);
        provider
            .commit_account_range(write, B256::ZERO, &first, Default::default(), Vec::new())
            .unwrap();

        // The same range again, and one skipping ahead of the cursor.
        let duplicate = provider.commit_account_range(
            write,
            B256::ZERO,
            &first,
            Default::default(),
            Vec::new(),
        );
        let skipped = verified_range(&accounts, 2..3, FAR, &[FAR]);
        let ahead =
            provider.commit_account_range(write, FAR, &skipped, Default::default(), Vec::new());

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

        let no_storage = provider.commit_account_range(
            write,
            B256::ZERO,
            &range,
            Default::default(),
            bytecodes.clone(),
        );
        assert!(
            matches!(no_storage, Err(SnapSyncError::MissingStorage { account }) if account == key(2))
        );

        let no_code =
            provider.commit_account_range(write, B256::ZERO, &range, storages, Vec::new());
        assert!(matches!(no_code, Err(SnapSyncError::MissingCode { .. })));

        let stray = B256Map::from_iter([(key(1), storage().1), (key(2), storage().1)]);
        let unexpected = provider.commit_account_range(write, B256::ZERO, &range, stray, bytecodes);
        assert!(
            matches!(unexpected, Err(SnapSyncError::UnexpectedStorage { account }) if account == key(1))
        );

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

        let coverage =
            provider.commit_account_range(write, B256::ZERO, &range, storages, Vec::new()).unwrap();

        assert!(coverage.is_complete());
    }

    #[test]
    fn a_write_from_a_replaced_attempt_changes_nothing() {
        let accounts = accounts();
        let (factory, replaced, generation) = started(&accounts);
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        let (storages, bytecodes) = dependencies();
        let provider = factory.database_provider_rw().unwrap();
        let current = provider.start_snap_attempt(generation).unwrap();

        let refused =
            provider.commit_account_range(replaced, B256::ZERO, &range, storages, bytecodes);

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
    fn a_range_proved_against_another_root_is_refused() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let other = vec![(key(7), account(7))];
        let range = verified_range(&other, 0..1, B256::ZERO, &[]);

        let refused = provider.commit_account_range(
            write,
            B256::ZERO,
            &range,
            Default::default(),
            Vec::new(),
        );

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
            provider.commit_account_range(
                write,
                B256::ZERO,
                &range,
                Default::default(),
                Vec::new()
            ),
            Err(SnapSyncError::NoCoverage)
        ));
    }

    #[test]
    fn coverage_survives_reopening_the_database() {
        let accounts = accounts();
        let (factory, write, _) = started(&accounts);
        let range = verified_range(&accounts, 0..1, B256::ZERO, &[key(1)]);
        let provider = factory.database_provider_rw().unwrap();
        let coverage = provider
            .commit_account_range(write, B256::ZERO, &range, Default::default(), Vec::new())
            .unwrap();
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
