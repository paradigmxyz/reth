//! Persists contract storage response by response, ahead of the account range committing to it.
//!
//! Progress is tied to the attempt, its pivot and the range, so slots proved against a superseded
//! root are never resumed.

use crate::{common::SnapRecord, SnapAccountStore, SnapAttemptStore, SnapSyncError, SnapWrite};
use alloy_primitives::{B256, U256};
use reth_db_api::{
    cursor::DbDupCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_storage_api::{DBProvider, MetadataProvider, MetadataWriter, SnapAttemptId, StateWriter};
use reth_trie_common::{root::storage_root, HashedPostState, HashedStorage, EMPTY_ROOT_HASH};
use serde::{Deserialize, Serialize};

/// Persistence for contract storage downloaded ahead of its account range.
///
/// Blanket-implemented over the node's writers, so slots and progress join the caller's
/// transaction and commit together or not at all.
pub trait SnapStorageStore {
    /// Returns how far the storage of the range requested from `origin` is persisted.
    fn storage_progress(
        &self,
        write: SnapWrite,
        origin: B256,
    ) -> Result<StorageProgress, SnapSyncError>;

    /// Persists `chunk` for the range requested from `origin`, where the coverage must continue.
    ///
    /// A contract starts at the zero slot, dropping whatever an earlier attempt left for it, and
    /// otherwise continues only where its last chunk ended.
    fn commit_storage_chunk(
        &self,
        write: SnapWrite,
        origin: B256,
        chunk: StorageChunk,
    ) -> Result<StorageProgress, SnapSyncError>
    where
        Self: MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>;
}

/// Verified slots of one contract, from the slot they were requested at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageChunk {
    // Hashed address of the contract.
    account: B256,
    // Root the slots were proved against.
    storage_root: B256,
    // Slot the chunk was requested from.
    from: B256,
    // Non-zero slots in key order.
    slots: Vec<(B256, U256)>,
    // Slot the storage continues at, or none once the trie is exhausted.
    next: Option<B256>,
}

impl StorageChunk {
    /// Creates a chunk of `account`'s storage requested from `from` and continuing at `next`.
    pub const fn new(
        account: B256,
        storage_root: B256,
        from: B256,
        slots: Vec<(B256, U256)>,
        next: Option<B256>,
    ) -> Self {
        Self { account, storage_root, from, slots, next }
    }
}

/// How far the contracts of the account range being downloaded have their storage persisted.
///
/// Contracts complete in key order, so only one is ever part way through.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StorageProgress {
    // Contracts up to this key have all their storage persisted.
    complete: Option<B256>,
    // The contract part way through, if any.
    partial: Option<PartialStorage>,
}

impl StorageProgress {
    /// No storage persisted yet.
    pub const START: Self = Self { complete: None, partial: None };

    /// Returns whether all of `account`'s storage is persisted.
    pub fn is_complete(&self, account: B256) -> bool {
        self.complete.is_some_and(|complete| account <= complete)
    }

    /// Slot `account`'s storage resumes at: zero before it starts, `None` once it is complete.
    pub fn resume_at(&self, account: B256) -> Option<B256> {
        if self.is_complete(account) {
            return None
        }
        let partial = self.partial.filter(|partial| partial.account == account);
        Some(partial.map_or(B256::ZERO, |partial| partial.next))
    }

    // The progress after `chunk`, which must continue its contract or start the next one.
    fn advance(&self, chunk: &StorageChunk) -> Result<Self, SnapSyncError> {
        let another = self.partial.is_some_and(|partial| partial.account != chunk.account);
        if another || self.resume_at(chunk.account) != Some(chunk.from) {
            return Err(SnapSyncError::OutOfOrderStorage {
                account: chunk.account,
                from: chunk.from,
            })
        }
        if let Some(partial) = self.partial &&
            partial.storage_root != chunk.storage_root
        {
            return Err(SnapSyncError::StorageRootMismatch {
                account: chunk.account,
                expected: partial.storage_root,
                got: chunk.storage_root,
            })
        }
        if chunk.next.is_some_and(|next| next <= chunk.from) {
            return Err(SnapSyncError::NoProgress { origin: chunk.from })
        }
        Ok(match chunk.next {
            Some(next) => Self {
                complete: self.complete,
                partial: Some(PartialStorage {
                    account: chunk.account,
                    storage_root: chunk.storage_root,
                    next,
                }),
            },
            None => Self { complete: Some(chunk.account), partial: None },
        })
    }
}

// A contract whose storage is persisted up to, but not including, `next`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PartialStorage {
    // Hashed address of the contract.
    account: B256,
    // Root its slots were proved against.
    storage_root: B256,
    // Slot its storage resumes at.
    next: B256,
}

// The progress record as persisted, tied to the write and range that recorded it.
#[derive(Serialize, Deserialize)]
struct StoredProgress {
    // Encoding version, checked before the rest is decoded.
    version: u32,
    // Attempt the slots belong to.
    attempt: SnapAttemptId,
    // Pivot generation the slots were proved against.
    state_version: u64,
    // Key the account range was requested from.
    origin: B256,
    // How far that range's contracts have got.
    progress: StorageProgress,
}

impl SnapRecord for StoredProgress {
    const KEY: &'static str = "snap_storage_progress";
    const VERSION: u32 = 1;
}

impl StoredProgress {
    // `progress` for `write`'s range at `origin` at this build's version.
    const fn new(write: SnapWrite, origin: B256, progress: StorageProgress) -> Self {
        Self {
            version: Self::VERSION,
            attempt: write.attempt(),
            state_version: write.state_version(),
            origin,
            progress,
        }
    }

    fn belongs_to(&self, write: SnapWrite, origin: B256) -> bool {
        self.attempt == write.attempt() &&
            self.state_version == write.state_version() &&
            self.origin == origin
    }
}

impl<T: MetadataProvider> SnapStorageStore for T {
    // A record left by another attempt, pivot or range reads as nothing persisted.
    fn storage_progress(
        &self,
        write: SnapWrite,
        origin: B256,
    ) -> Result<StorageProgress, SnapSyncError> {
        self.authorize_snap_write(write)?;
        let Some(stored) = StoredProgress::read(self)? else { return Ok(StorageProgress::START) };
        Ok(if stored.belongs_to(write, origin) { stored.progress } else { StorageProgress::START })
    }

    // Every check runs before the first write, so a refused chunk changes nothing.
    fn commit_storage_chunk(
        &self,
        write: SnapWrite,
        origin: B256,
        chunk: StorageChunk,
    ) -> Result<StorageProgress, SnapSyncError>
    where
        Self: MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>,
    {
        let coverage = self.account_coverage(write)?.ok_or(SnapSyncError::NoCoverage)?;
        if coverage.next() != Some(origin) {
            return Err(SnapSyncError::OutOfOrderRange { expected: coverage.next(), got: origin })
        }
        if chunk.account < origin || chunk.storage_root == EMPTY_ROOT_HASH {
            return Err(SnapSyncError::UnexpectedStorage { account: chunk.account })
        }
        let progress = self.storage_progress(write, origin)?.advance(&chunk)?;

        if chunk.from == B256::ZERO {
            self.remove::<tables::HashedStorages>(chunk.account..=chunk.account)?;
        }
        let state = HashedPostState::default()
            .with_storages([(chunk.account, HashedStorage::from_iter(chunk.slots))])
            .into_sorted();
        self.write_hashed_state(&state)?;
        StoredProgress::new(write, origin, progress).write(self)?;
        Ok(progress)
    }
}

/// Root of the storage persisted for `account`, streamed so a large contract is never held in
/// memory.
pub(crate) fn persisted_storage_root(tx: &impl DbTx, account: B256) -> Result<B256, SnapSyncError> {
    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
    let mut failed = None;
    let slots = cursor.walk_dup(Some(account), None)?.map_while(|entry| {
        entry.map(|(_, slot)| (slot.key, slot.value)).map_err(|error| failed = Some(error)).ok()
    });
    let root = storage_root(slots);
    failed.map_or(Ok(root), |error| Err(error.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        account, generation, hashed_factory, state_root, storage_root_of, stored_slots,
    };
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, DatabaseProviderFactory, ProviderFactory,
    };
    use std::ops::Range;

    const CONTRACT: B256 = B256::repeat_byte(0x22);

    fn slot(value: u8) -> B256 {
        B256::with_last_byte(value)
    }

    fn slots() -> Vec<(B256, U256)> {
        vec![(slot(1), U256::from(11)), (slot(2), U256::from(12)), (slot(3), U256::from(13))]
    }

    fn root() -> B256 {
        storage_root_of(&slots())
    }

    // `slots()[served]` requested from `from`, continuing at `next`.
    fn chunk(served: Range<usize>, from: B256, next: Option<B256>) -> StorageChunk {
        StorageChunk::new(CONTRACT, root(), from, slots()[served].to_vec(), next)
    }

    // An attempt whose account coverage has not reached the contract yet.
    fn started() -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite) {
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();
        let mut contract = account(1);
        contract.storage_root = root();
        let generation = generation(1, state_root(&[(CONTRACT, contract)]));
        let write = provider.start_snap_attempt(generation).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    #[test]
    fn an_interrupted_chunk_commit_keeps_the_last_committed_progress() {
        let (factory, write) = started();
        let provider = factory.database_provider_rw().unwrap();
        let first = chunk(0..1, B256::ZERO, Some(slot(2)));
        let progress = provider.commit_storage_chunk(write, B256::ZERO, first).unwrap();
        provider.commit().unwrap();
        assert_eq!(progress.resume_at(CONTRACT), Some(slot(2)));

        let provider = factory.database_provider_rw().unwrap();
        let rest = chunk(1..3, slot(2), None);
        assert!(provider
            .commit_storage_chunk(write, B256::ZERO, rest)
            .unwrap()
            .is_complete(CONTRACT));
        drop(provider);

        let provider = factory.database_provider_rw().unwrap();
        assert_eq!(provider.storage_progress(write, B256::ZERO).unwrap(), progress);
        assert_eq!(stored_slots(&provider, CONTRACT), slots()[..1]);
    }

    #[test]
    fn a_chunk_must_continue_its_contract_before_another_starts() {
        let (factory, write) = started();
        let provider = factory.database_provider_rw().unwrap();
        let first = chunk(0..1, B256::ZERO, Some(slot(2)));
        let progress = provider.commit_storage_chunk(write, B256::ZERO, first).unwrap();

        let refused = [
            // Restarted, skipping ahead, or another contract while this one is part way through.
            chunk(0..1, B256::ZERO, Some(slot(2))),
            chunk(2..3, slot(3), None),
            StorageChunk::new(B256::repeat_byte(0x44), root(), B256::ZERO, slots(), None),
        ];
        for chunk in refused {
            assert!(matches!(
                provider.commit_storage_chunk(write, B256::ZERO, chunk),
                Err(SnapSyncError::OutOfOrderStorage { .. })
            ));
        }
        let stalled = chunk(1..1, slot(2), Some(slot(2)));
        assert!(matches!(
            provider.commit_storage_chunk(write, B256::ZERO, stalled),
            Err(SnapSyncError::NoProgress { .. })
        ));
        let other_root = StorageChunk::new(
            CONTRACT,
            B256::repeat_byte(0x33),
            slot(2),
            slots()[1..].to_vec(),
            None,
        );
        assert!(matches!(
            provider.commit_storage_chunk(write, B256::ZERO, other_root),
            Err(SnapSyncError::StorageRootMismatch { .. })
        ));

        assert_eq!(provider.storage_progress(write, B256::ZERO).unwrap(), progress);
        assert_eq!(stored_slots(&provider, CONTRACT), slots()[..1]);
    }

    #[test]
    fn starting_a_contract_replaces_what_an_earlier_attempt_left() {
        let (factory, write) = started();
        let provider = factory.database_provider_rw().unwrap();
        let leftover = HashedStorage::from_iter([(slot(7), U256::from(77))]);
        let leftover = HashedPostState::default().with_storages([(CONTRACT, leftover)]);
        provider.write_hashed_state(&leftover.into_sorted()).unwrap();

        provider.commit_storage_chunk(write, B256::ZERO, chunk(0..3, B256::ZERO, None)).unwrap();

        assert_eq!(stored_slots(&provider, CONTRACT), slots());
        assert_eq!(persisted_storage_root(provider.tx_ref(), CONTRACT).unwrap(), root());
    }

    #[test]
    fn progress_belongs_to_the_pivot_and_range_it_was_recorded_for() {
        let (factory, write) = started();
        let provider = factory.database_provider_rw().unwrap();
        let first = chunk(0..1, B256::ZERO, Some(slot(2)));
        provider.commit_storage_chunk(write, B256::ZERO, first).unwrap();

        assert_eq!(provider.storage_progress(write, slot(9)).unwrap(), StorageProgress::START);

        // Slots proved against the previous root are not resumed once the pivot moves.
        let moved = generation(2, B256::repeat_byte(0xcc));
        let advanced = provider.advance_snap_pivot(write, moved).unwrap();
        assert_eq!(
            provider.storage_progress(advanced, B256::ZERO).unwrap(),
            StorageProgress::START
        );
        assert!(matches!(
            provider.commit_storage_chunk(write, B256::ZERO, chunk(1..3, slot(2), None)),
            Err(SnapSyncError::StaleWrite { .. })
        ));
    }

    #[test]
    fn storage_outside_the_range_being_downloaded_is_refused() {
        let (factory, write) = started();
        let provider = factory.database_provider_rw().unwrap();

        let ahead = provider.commit_storage_chunk(write, slot(9), chunk(0..3, B256::ZERO, None));
        let empty = StorageChunk::new(CONTRACT, EMPTY_ROOT_HASH, B256::ZERO, Vec::new(), None);
        let empty = provider.commit_storage_chunk(write, B256::ZERO, empty);

        assert!(matches!(ahead, Err(SnapSyncError::OutOfOrderRange { .. })));
        assert!(matches!(empty, Err(SnapSyncError::UnexpectedStorage { .. })));
        assert!(stored_slots(&provider, CONTRACT).is_empty());
    }

    #[test]
    fn a_record_this_build_cannot_read_is_reported() {
        let (factory, write) = started();

        for record in [br#"{"version":999}"#.to_vec(), b"{}".to_vec()] {
            let provider = factory.database_provider_rw().unwrap();
            provider.write_metadata(StoredProgress::KEY, record).unwrap();

            assert!(matches!(
                provider.storage_progress(write, B256::ZERO),
                Err(SnapSyncError::UnsupportedRecord { .. })
            ));
        }
    }
}
