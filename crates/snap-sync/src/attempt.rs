//! Persistence for the attempt that owns downloaded snap state.
//!
//! Snap writes land in the canonical hashed state tables, so nothing in the data says which
//! attempt produced it. Every write presents a [`SnapWrite`]; ones that no longer match are
//! refused.

use crate::{CatchUpProgress, SnapGeneration, SnapSyncError};
use reth_storage_api::{
    BlockHashReader, MetadataProvider, MetadataWriter, SnapAttempt, SnapAttemptId, StorageSettings,
};

/// Persistence for the attempt that owns downloaded snap state.
///
/// Blanket-implemented over node metadata access, so these writes join the caller's transaction:
/// state, bytecode and the attempt record commit together or not at all.
pub trait SnapAttemptStore {
    /// Starts an attempt anchored to `generation`, superseding any already recorded, with its
    /// catch-up progress at that pivot.
    fn start_snap_attempt(&self, generation: SnapGeneration) -> Result<SnapWrite, SnapSyncError>
    where
        Self: MetadataWriter;

    /// Returns the write an unfinished attempt accepts, if one owns the persisted state.
    fn active_snap_write(&self) -> Result<Option<SnapWrite>, SnapSyncError>;

    /// Returns the attempt when `write` still owns the persisted state, rejecting it otherwise.
    fn authorize_snap_write(&self, write: SnapWrite) -> Result<SnapAttempt, SnapSyncError>;

    /// Returns the attempt when `write` owns its state and its pivot is still canonical.
    ///
    /// Downloaded ranges are proved against the pivot, so they describe the fork it sits on, not
    /// the canonical chain the applied blocks were checked against.
    fn authorize_canonical_snap_write(
        &self,
        write: SnapWrite,
    ) -> Result<SnapAttempt, SnapSyncError>
    where
        Self: BlockHashReader;

    /// Re-anchors the attempt to `generation`, refusing writes proved against the previous root.
    ///
    /// Both pivots must be canonical and the new one past the old, so it descends from all
    /// downloaded state. An orphaned pivot requires recovery or a fresh attempt.
    fn advance_snap_pivot(
        &self,
        write: SnapWrite,
        generation: SnapGeneration,
    ) -> Result<SnapWrite, SnapSyncError>
    where
        Self: MetadataWriter + BlockHashReader;

    /// Marks the attempt's downloaded state verified.
    fn verify_snap_attempt(&self, write: SnapWrite) -> Result<(), SnapSyncError>
    where
        Self: MetadataWriter;

    /// Gives up on an unfinished attempt, refusing its outstanding writes.
    fn abandon_snap_attempt(&self) -> Result<(), SnapSyncError>
    where
        Self: MetadataWriter;
}

/// What a write presents to prove it belongs to the attempt owning the persisted state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapWrite {
    // Attempt this write belongs to.
    attempt: SnapAttemptId,
    // Pivot generation within that attempt.
    state_version: u64,
}

impl SnapWrite {
    // What `attempt` currently accepts.
    const fn of(attempt: &SnapAttempt) -> Self {
        Self { attempt: attempt.id(), state_version: attempt.state_version() }
    }

    /// Attempt this write belongs to.
    pub const fn attempt(&self) -> SnapAttemptId {
        self.attempt
    }

    /// Pivot generation this write was proved against.
    pub const fn state_version(&self) -> u64 {
        self.state_version
    }
}

impl<T: MetadataProvider> SnapAttemptStore for T {
    fn start_snap_attempt(&self, generation: SnapGeneration) -> Result<SnapWrite, SnapSyncError>
    where
        Self: MetadataWriter,
    {
        // Absent settings mean the legacy layout.
        if !self.storage_settings()?.unwrap_or_else(StorageSettings::v1).use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorage)
        }

        let attempt =
            SnapAttempt::start(self.snap_attempt()?, generation.target(), generation.state_root());
        self.write_snap_attempt(&attempt)?;
        // Ranges committed at this pivot need every list after it, however far the pivot moves.
        CatchUpProgress::at_pivot(attempt.pivot()).write(self, attempt.id())?;
        Ok(SnapWrite::of(&attempt))
    }

    fn active_snap_write(&self) -> Result<Option<SnapWrite>, SnapSyncError> {
        Ok(self.snap_attempt()?.filter(SnapAttempt::is_unfinished).as_ref().map(SnapWrite::of))
    }

    fn authorize_snap_write(&self, write: SnapWrite) -> Result<SnapAttempt, SnapSyncError> {
        let attempt = self.snap_attempt()?.ok_or(SnapSyncError::NoAttempt)?;
        if !attempt.is_unfinished() || SnapWrite::of(&attempt) != write {
            return Err(SnapSyncError::StaleWrite {
                attempt: write.attempt,
                state_version: write.state_version,
            })
        }
        Ok(attempt)
    }

    fn authorize_canonical_snap_write(&self, write: SnapWrite) -> Result<SnapAttempt, SnapSyncError>
    where
        Self: BlockHashReader,
    {
        let attempt = self.authorize_snap_write(write)?;
        let pivot = attempt.pivot();
        if self.block_hash(pivot.number)? != Some(pivot.hash) {
            return Err(SnapSyncError::NonCanonicalBlock { block: pivot.number, hash: pivot.hash })
        }
        Ok(attempt)
    }

    fn advance_snap_pivot(
        &self,
        write: SnapWrite,
        generation: SnapGeneration,
    ) -> Result<SnapWrite, SnapSyncError>
    where
        Self: MetadataWriter + BlockHashReader,
    {
        // Replacing an orphaned pivot would hide the fork that downloaded ranges belong to.
        let mut attempt = self.authorize_canonical_snap_write(write)?;
        let (pivot, target) = (attempt.pivot(), generation.target());
        // Ranges already committed hold the old pivot's changes, which no list can undo.
        if target.number <= pivot.number {
            return Err(SnapSyncError::PivotNotAdvanced {
                pivot: pivot.number,
                target: target.number,
            })
        }
        if self.block_hash(target.number)? != Some(target.hash) {
            return Err(SnapSyncError::NonCanonicalBlock { block: target.number, hash: target.hash })
        }
        attempt.re_anchor(target, generation.state_root());
        self.write_snap_attempt(&attempt)?;
        Ok(SnapWrite::of(&attempt))
    }

    fn verify_snap_attempt(&self, write: SnapWrite) -> Result<(), SnapSyncError>
    where
        Self: MetadataWriter,
    {
        let mut attempt = self.authorize_snap_write(write)?;
        attempt.verify();
        self.write_snap_attempt(&attempt)?;
        Ok(())
    }

    fn abandon_snap_attempt(&self) -> Result<(), SnapSyncError>
    where
        Self: MetadataWriter,
    {
        if let Some(mut attempt) = self.snap_attempt()? &&
            attempt.is_unfinished()
        {
            attempt.abandon();
            self.write_snap_attempt(&attempt)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::insert_generation_headers;
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{Bytes, B256};
    use reth_db_api::{tables, transaction::DbTx};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DBProvider, DatabaseProviderFactory, ProviderFactory,
    };
    use reth_storage_api::{metadata::keys, StateWriter};
    use reth_trie_common::HashedPostState;
    use revm::{bytecode::Bytecode, database::states::StateChangeset};

    const HASHED_ADDRESS: B256 = B256::repeat_byte(0xbb);

    fn code() -> Bytecode {
        Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]))
    }

    fn generation(block: u64) -> SnapGeneration {
        SnapGeneration::new(
            BlockNumHash::new(block, B256::repeat_byte(block as u8)),
            B256::repeat_byte(0xaa),
        )
    }

    // A database using the hashed state layout snap writes into, holding the blocks the
    // generations above anchor to.
    fn factory() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v2()).unwrap();
        provider.commit().unwrap();
        insert_generation_headers(&factory);
        factory
    }

    // Persists one account and its bytecode through the writers snap shares with execution.
    fn download(provider: &impl StateWriter) {
        let mut state = HashedPostState::default();
        state.accounts.insert(HASHED_ADDRESS, Some(Account::default()));
        provider.write_hashed_state(&state.into_sorted()).unwrap();
        provider
            .write_state_changes(StateChangeset {
                contracts: vec![(code().hash_slow(), code())],
                ..Default::default()
            })
            .unwrap();
    }

    fn downloaded(provider: &impl DBProvider) -> (bool, bool) {
        let tx = provider.tx_ref();
        (
            tx.get::<tables::HashedAccounts>(HASHED_ADDRESS).unwrap().is_some(),
            tx.get::<tables::Bytecodes>(code().hash_slow()).unwrap().is_some(),
        )
    }

    #[test]
    fn nothing_owns_the_state_before_an_attempt_starts() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();

        assert_eq!(provider.active_snap_write().unwrap(), None);
        let write = SnapWrite { attempt: SnapAttemptId::FIRST, state_version: 0 };
        assert!(matches!(provider.authorize_snap_write(write), Err(SnapSyncError::NoAttempt)));
    }

    #[test]
    fn address_keyed_state_cannot_host_an_attempt() {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();

        // Nothing recorded means the legacy layout.
        assert!(matches!(
            provider.start_snap_attempt(generation(1)),
            Err(SnapSyncError::UnsupportedStorage)
        ));
        provider.write_storage_settings(StorageSettings::v1()).unwrap();
        assert!(matches!(
            provider.start_snap_attempt(generation(1)),
            Err(SnapSyncError::UnsupportedStorage)
        ));
        assert_eq!(provider.snap_attempt().unwrap(), None);
    }

    #[test]
    fn restarting_at_the_same_pivot_takes_a_new_identity() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();

        let first = provider.start_snap_attempt(generation(1)).unwrap();
        let second = provider.start_snap_attempt(generation(1)).unwrap();

        assert_ne!(first.attempt(), second.attempt());
        // The superseded attempt's outstanding downloads no longer own the state.
        assert!(matches!(
            provider.authorize_snap_write(first),
            Err(SnapSyncError::StaleWrite { .. })
        ));
        provider.authorize_snap_write(second).unwrap();
    }

    #[test]
    fn abandoning_an_attempt_keeps_its_identity_taken() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();

        let abandoned = provider.start_snap_attempt(generation(1)).unwrap();
        provider.abandon_snap_attempt().unwrap();
        assert_eq!(provider.active_snap_write().unwrap(), None);
        let started = provider.start_snap_attempt(generation(2)).unwrap();

        assert_ne!(abandoned.attempt(), started.attempt());
        // The abandoned attempt's outstanding downloads cannot pass as the new attempt's work.
        assert!(matches!(
            provider.authorize_snap_write(abandoned),
            Err(SnapSyncError::StaleWrite { .. })
        ));
    }

    #[test]
    fn the_attempt_survives_reopening_the_database() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(7)).unwrap();
        provider.commit().unwrap();

        let reopened = factory.database_provider_rw().unwrap();

        assert_eq!(reopened.active_snap_write().unwrap(), Some(write));
        let attempt = reopened.snap_attempt().unwrap().unwrap();
        assert_eq!(attempt.pivot(), BlockNumHash::new(7, B256::repeat_byte(7)));
        assert_eq!(attempt.state_root(), B256::repeat_byte(0xaa));
        assert!(attempt.is_unfinished());
    }

    #[test]
    fn committing_keeps_downloaded_state_and_progress_together() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1)).unwrap();
        download(&provider);
        provider.commit().unwrap();

        let provider = factory.database_provider_rw().unwrap();
        assert_eq!(provider.active_snap_write().unwrap(), Some(write));
        assert_eq!(downloaded(&provider), (true, true));
    }

    #[test]
    fn rolling_back_drops_downloaded_state_and_progress_together() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        provider.start_snap_attempt(generation(1)).unwrap();
        download(&provider);
        // Dropping without committing is the interrupted-commit case.
        drop(provider);

        let provider = factory.database_provider_rw().unwrap();
        assert_eq!(provider.active_snap_write().unwrap(), None);
        assert_eq!(downloaded(&provider), (false, false));
    }

    #[test]
    fn advancing_the_pivot_rejects_writes_proved_against_the_old_root() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();

        let before = provider.start_snap_attempt(generation(1)).unwrap();
        let after = provider.advance_snap_pivot(before, generation(2)).unwrap();

        assert_eq!(after.attempt(), before.attempt());
        assert_eq!(after.state_version(), before.state_version() + 1);
        assert!(matches!(
            provider.authorize_snap_write(before),
            Err(SnapSyncError::StaleWrite { .. })
        ));
        provider.authorize_snap_write(after).unwrap();
    }

    #[test]
    fn the_pivot_advances_repeatedly_within_one_attempt() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let first = provider.start_snap_attempt(generation(1)).unwrap();

        let second = provider.advance_snap_pivot(first, generation(2)).unwrap();
        let third = provider.advance_snap_pivot(second, generation(3)).unwrap();

        assert_eq!(third.attempt(), first.attempt());
        assert_eq!(third.state_version(), first.state_version() + 2);
        assert_eq!(provider.snap_attempt().unwrap().unwrap().pivot().number, 3);
        for stale in [first, second] {
            assert!(matches!(
                provider.authorize_snap_write(stale),
                Err(SnapSyncError::StaleWrite { .. })
            ));
        }
    }

    #[test]
    fn the_pivot_only_moves_forward() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(2)).unwrap();

        // Committed ranges hold block 2's changes, which a lower pivot's lists cannot undo.
        for target in [1, 2] {
            assert!(matches!(
                provider.advance_snap_pivot(write, generation(target)),
                Err(SnapSyncError::PivotNotAdvanced { pivot: 2, target: refused })
                    if refused == target
            ));
        }
        provider.authorize_snap_write(write).unwrap();
    }

    #[test]
    fn the_pivot_cannot_move_off_the_canonical_chain() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1)).unwrap();
        let orphan = SnapGeneration::new(BlockNumHash::new(2, B256::repeat_byte(0xee)), B256::ZERO);

        let refused = provider.advance_snap_pivot(write, orphan);

        assert!(matches!(refused, Err(SnapSyncError::NonCanonicalBlock { block: 2, .. })));
        provider.authorize_snap_write(write).unwrap();
    }

    #[test]
    fn an_interrupted_advance_keeps_the_previous_pivot() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1)).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_rw().unwrap();
        provider.advance_snap_pivot(write, generation(2)).unwrap();
        drop(provider);

        // Downloads still in flight against the old root keep committing.
        let provider = factory.database_provider_rw().unwrap();
        assert_eq!(provider.active_snap_write().unwrap(), Some(write));
        assert_eq!(provider.authorize_snap_write(write).unwrap().pivot().number, 1);
    }

    #[test]
    fn a_verified_attempt_accepts_no_further_writes() {
        let factory = factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1)).unwrap();

        provider.verify_snap_attempt(write).unwrap();

        assert!(!provider.snap_attempt().unwrap().unwrap().is_unfinished());
        assert_eq!(provider.active_snap_write().unwrap(), None);
        assert!(matches!(
            provider.authorize_snap_write(write),
            Err(SnapSyncError::StaleWrite { .. })
        ));
        // Verified state is complete, so abandoning cannot turn it into leftovers.
        provider.abandon_snap_attempt().unwrap();
        assert!(provider.snap_attempt().unwrap().unwrap().is_verified());
    }

    #[test]
    fn a_record_this_build_cannot_read_is_reported_rather_than_ignored() {
        let factory = factory();

        for record in [br#"{"version":999}"#.to_vec(), b"{}".to_vec(), b"not json".to_vec()] {
            let provider = factory.database_provider_rw().unwrap();
            provider.write_metadata(keys::SNAP_ATTEMPT, record).unwrap();

            assert!(provider.snap_attempt().is_err());
            // Starting a new attempt cannot guarantee a distinct identity over an unreadable one.
            assert!(provider.start_snap_attempt(generation(1)).is_err());
        }
    }
}
