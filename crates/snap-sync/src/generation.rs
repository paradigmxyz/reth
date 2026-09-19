//! Tracks the bootstrap phase alongside the attempt's authoritative metadata.
//!
//! Pivot identity, account coverage and applied BAL progress come from their domain stores.
//! The stage marker retains the phase until trie verification and pipeline publication finish.

use crate::{
    error::db_error, handoff::publish_state_snapshot, SnapAccountStore, SnapAttemptStore,
    SnapCatchUpStore, SnapStateVerifier, SnapSyncError,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use alloy_rlp::{Decodable, Encodable};
use reth_db_api::{tables, transaction::DbTxMut};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::{DatabaseProviderFactory, StaticFileProviderFactory};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockHashReader, DBProvider, HeaderProvider, MetadataProvider, MetadataWriter,
    PruneCheckpointWriter, StageCheckpointReader, StageCheckpointWriter, StorageSettingsCache,
};

/// A snap attempt's canonical pivot identity and the state root authenticating its downloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapGeneration {
    // Pivot block this attempt is anchored to.
    target: BlockNumHash,
    // Root downloaded ranges authenticate against.
    state_root: B256,
    // Stage reached so far.
    phase: SnapPhase,
}

impl SnapGeneration {
    /// Creates a generation anchored to the given pivot, before any range is downloaded.
    pub const fn new(target: BlockNumHash, state_root: B256) -> Self {
        Self { target, state_root, phase: SnapPhase::Accounts }
    }

    /// Pivot block this generation is anchored to.
    pub const fn target(&self) -> BlockNumHash {
        self.target
    }

    /// State root that downloaded ranges authenticate against.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Stage this generation has reached.
    pub const fn phase(&self) -> SnapPhase {
        self.phase
    }

    /// Returns how far the canonical head has moved past this generation's anchor.
    pub const fn lag(&self, head: u64) -> u64 {
        head.saturating_sub(self.target.number)
    }

    /// Returns whether the pivot remains canonical, independently of recovery eligibility.
    pub fn is_canonical(&self, provider: &impl HeaderProvider) -> Result<bool, SnapSyncError> {
        let header = provider.sealed_header(self.target.number)?;
        Ok(header.is_some_and(|header| header.hash() == self.target.hash))
    }

    // Returns this generation moved to `phase`.
    pub(crate) const fn with_phase(mut self, phase: SnapPhase) -> Self {
        self.phase = phase;
        self
    }
}

/// The stage a [`SnapGeneration`] has reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapPhase {
    /// Account, storage and bytecode ranges are being downloaded.
    Accounts,
    /// Authenticated block access lists are being applied.
    BlockAccessLists,
    /// The final state trie is being rebuilt and checked.
    Trie,
}

// Existing stage tooling can inspect the generation without a Snap-specific table.
pub(crate) const SNAP_SYNC_STAGE: StageId = StageId::Other("SnapSync");
// Versioning prevents an incompatible restart marker from being reinterpreted.
const SNAP_GENERATION_VERSION: u8 = 3;

/// Owns durable state-generation writes for one provider factory.
#[derive(Debug)]
pub struct SnapStateStore<'a, F> {
    // The store opens one short transaction per durable transition.
    pub(crate) factory: &'a F,
}

impl<'a, F> SnapStateStore<'a, F> {
    /// Creates a store over `factory`.
    pub const fn new(factory: &'a F) -> Self {
        Self { factory }
    }

    /// Starts a clean generation, replacing unfinished work but refusing executed or completed
    /// state.
    pub fn begin_generation(&self, generation: SnapDownloadProgress) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider<Tx: DbTxMut>
            + StageCheckpointReader
            + StageCheckpointWriter
            + StorageSettingsCache
            + MetadataProvider
            + MetadataWriter,
    {
        generation.validate()?;
        generation.ensure_phase(SnapPhase::Accounts)?;
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorage);
        }
        // Check in the transaction that clears state, even when the caller checked before
        // selecting a pivot. A completed bootstrap must never become a fresh generation.
        if !Self::requires_bootstrap_in(&provider)? {
            return Err(SnapSyncError::ExistingState);
        }

        let write = provider.start_snap_attempt(generation.generation())?;
        provider.start_account_coverage(write)?;
        let tx = provider.tx_ref();
        tx.clear::<tables::HashedAccounts>().map_err(db_error)?;
        tx.clear::<tables::HashedStorages>().map_err(db_error)?;
        tx.clear::<tables::AccountsTrie>().map_err(db_error)?;
        tx.clear::<tables::StoragesTrie>().map_err(db_error)?;
        provider
            .save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::default())
            .map_err(db_error)?;
        provider
            .save_stage_checkpoint_progress(StageId::MerkleExecute, Vec::new())
            .map_err(db_error)?;
        Self::save_generation(&provider, generation)?;
        provider.commit().map_err(db_error)
    }

    // Clears the restart marker only after the canonical root and Merkle checkpoint agree, and
    // publishes the frontier in the same transaction so a crash can never leave an accepted state
    // that the pipeline would resume from genesis.
    pub(crate) fn finish_generation(
        &self,
        generation: SnapDownloadProgress,
    ) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider<Tx: DbTxMut>
            + HeaderProvider
            + BlockHashReader
            + PruneCheckpointWriter
            + StageCheckpointReader
            + StageCheckpointWriter
            + StorageSettingsCache
            + StaticFileProviderFactory
            + MetadataProvider
            + MetadataWriter,
    {
        generation.validate()?;
        generation.ensure_phase(SnapPhase::Trie)?;
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorage);
        }
        self.ensure_generation(&provider, generation)?;
        let write = provider.active_snap_write()?.ok_or(SnapSyncError::NoAttempt)?;
        let verified = provider.verify_state_root(write)?;
        provider
            .save_stage_checkpoint(SNAP_SYNC_STAGE, StageCheckpoint::new(generation.target_block))
            .map_err(db_error)?;
        provider.save_stage_checkpoint_progress(SNAP_SYNC_STAGE, Vec::new()).map_err(db_error)?;
        publish_state_snapshot(&provider, verified.target().number)?;
        provider.commit().map_err(db_error)
    }

    // Stage progress keeps restart data visible to existing database tooling.
    pub(crate) fn save_generation(
        provider: &impl StageCheckpointWriter,
        generation: SnapDownloadProgress,
    ) -> Result<(), SnapSyncError> {
        provider
            .save_stage_checkpoint(SNAP_SYNC_STAGE, StageCheckpoint::new(generation.target_block))
            .map_err(db_error)?;
        provider
            .save_stage_checkpoint_progress(SNAP_SYNC_STAGE, alloy_rlp::encode(generation))
            .map_err(db_error)
    }

    // Empty progress remains compatible with stages that clear progress without deleting its row.
    fn load_generation(
        provider: &(impl StageCheckpointReader + MetadataProvider),
    ) -> Result<Option<SnapDownloadProgress>, SnapSyncError> {
        let Some(encoded) = provider
            .get_stage_checkpoint_progress(SNAP_SYNC_STAGE)
            .map_err(db_error)?
            .filter(|encoded| !encoded.is_empty())
        else {
            return Ok(None);
        };
        let mut generation = alloy_rlp::decode_exact::<SnapDownloadProgress>(&encoded)
            .map_err(|error| SnapSyncError::InvalidGeneration(error.to_string()))?;
        generation.validate()?;
        let write = provider.active_snap_write()?.ok_or(SnapSyncError::NoAttempt)?;
        let attempt = provider.authorize_snap_write(write)?;
        let coverage = provider.account_coverage(write)?.ok_or(SnapSyncError::NoCoverage)?;
        let progress =
            provider.catch_up_progress(write)?.ok_or(SnapSyncError::NoCatchUpProgress)?;
        generation.target_block = attempt.pivot().number;
        generation.target_hash = attempt.pivot().hash;
        generation.state_root = attempt.state_root();
        generation.next_account = coverage.next().unwrap_or(crate::MAX_HASH);
        generation.next_block = progress.next();
        if generation.phase == SnapPhase::Accounts && coverage.is_complete() {
            generation.phase = SnapPhase::BlockAccessLists;
        }
        generation.validate()?;
        if generation.phase == SnapPhase::Trie &&
            (!coverage.is_complete() || progress.applied() != attempt.pivot())
        {
            return Err(SnapSyncError::InvalidGeneration(
                "trie verification requires complete state at the pivot".into(),
            ))
        }
        Ok(Some(generation))
    }

    // Checking through the active transaction prevents stale work from crossing generations.
    pub(crate) fn ensure_generation(
        &self,
        provider: &(impl StageCheckpointReader + MetadataProvider),
        generation: SnapDownloadProgress,
    ) -> Result<(), SnapSyncError> {
        if Self::load_generation(provider)? == Some(generation) {
            Ok(())
        } else {
            Err(SnapSyncError::StaleGeneration)
        }
    }

    /// Re-anchors downloads before catching the covered prefix up to the new root.
    pub(crate) fn advance_generation(
        &self,
        generation: SnapDownloadProgress,
        target: u64,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: crate::SnapSyncProvider,
    {
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        self.ensure_generation(&provider, generation)?;
        if target > generation.target_block {
            let header = provider
                .sealed_header(target)?
                .ok_or(SnapSyncError::MissingHeader { block: target })?;
            let pivot =
                SnapGeneration::new(BlockNumHash::new(target, header.hash()), header.state_root());
            let write = provider.active_snap_write()?.ok_or(SnapSyncError::NoAttempt)?;
            provider.advance_snap_pivot(write, pivot)?;
        }
        let next = Self::load_generation(&provider)?.ok_or(SnapSyncError::StaleGeneration)?;
        Self::save_generation(&provider, next)?;
        provider.commit().map_err(db_error)?;
        Ok(next)
    }

    /// Enters trie verification only once coverage and applied BALs reach the same pivot.
    pub(crate) fn complete_block_access_lists(
        &self,
        generation: SnapDownloadProgress,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: crate::SnapSyncProvider,
    {
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        self.ensure_generation(&provider, generation)?;
        generation.ensure_phase(SnapPhase::BlockAccessLists)?;
        let write = provider.active_snap_write()?.ok_or(SnapSyncError::NoAttempt)?;
        let attempt = provider.authorize_canonical_snap_write(write)?;
        if !provider.account_coverage(write)?.is_some_and(|coverage| coverage.is_complete()) ||
            provider.catch_up_progress(write)?.map(|progress| progress.applied()) !=
                Some(attempt.pivot())
        {
            return Err(SnapSyncError::InvalidGeneration(
                "state coverage and BALs have not reached the pivot".into(),
            ));
        }
        let next = SnapDownloadProgress { phase: SnapPhase::Trie, ..generation };
        Self::save_generation(&provider, next)?;
        provider.commit().map_err(db_error)?;
        Ok(next)
    }
}

impl<F> SnapStateStore<'_, F>
where
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader + MetadataProvider,
{
    /// Returns whether the database is fresh or has an unfinished snapshot; does not check snap
    /// support.
    pub fn requires_bootstrap(&self) -> Result<bool, SnapSyncError> {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        Self::requires_bootstrap_in(&provider)
    }

    /// Returns the partial generation that must be resumed before state is served.
    pub fn interrupted_generation(&self) -> Result<Option<SnapDownloadProgress>, SnapSyncError> {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        Self::load_generation(&provider)
    }

    /// Returns the completed snapshot's state frontier, or `None` if unfinished or never snap
    /// synced.
    pub fn completed_block(&self) -> Result<Option<u64>, SnapSyncError> {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        Self::completed_block_in(&provider)
    }
}

impl<F> SnapStateStore<'_, F> {
    fn requires_bootstrap_in(
        provider: &(impl StageCheckpointReader + MetadataProvider),
    ) -> Result<bool, SnapSyncError> {
        if Self::completed_block_in(provider)?.is_some() {
            return Ok(false);
        }
        // Execution may have committed state before Finish advances, including during an
        // interrupted ordinary backfill. Genesis alone does not preclude snapshot bootstrap.
        for stage in [StageId::Execution, StageId::Finish] {
            if provider
                .get_stage_checkpoint(stage)
                .map_err(db_error)?
                .is_some_and(|checkpoint| checkpoint.block_number > 0)
            {
                if Self::load_generation(provider)?.is_some() {
                    return Err(SnapSyncError::ExistingState);
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    // An accepted generation leaves its block behind with no resumable progress.
    pub(crate) fn completed_block_in(
        provider: &(impl StageCheckpointReader + MetadataProvider),
    ) -> Result<Option<u64>, SnapSyncError> {
        if Self::load_generation(provider)?.is_some() {
            return Ok(None);
        }
        Ok(provider
            .get_stage_checkpoint(SNAP_SYNC_STAGE)
            .map_err(db_error)?
            .map(|checkpoint| checkpoint.block_number))
    }
}

/// Bootstrap progress reconstructed from the attempt, coverage and catch-up records.
#[derive(Clone, Copy, Debug, Eq, PartialEq, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
pub struct SnapDownloadProgress {
    // Rejects markers written by an incompatible schema.
    version: u8,
    /// Target block number.
    pub target_block: u64,
    /// Target block hash.
    pub target_hash: B256,
    /// State root authenticated by downloaded ranges.
    pub state_root: B256,
    /// Current assembly phase.
    pub phase: SnapPhase,
    /// Inclusive account origin for the next range request.
    pub next_account: B256,
    /// First block whose BAL has not been applied.
    pub next_block: u64,
}

impl SnapDownloadProgress {
    /// Creates a generation beginning at the first account hash.
    pub const fn new(target_block: u64, target_hash: B256, state_root: B256) -> Self {
        Self {
            version: SNAP_GENERATION_VERSION,
            target_block,
            target_hash,
            state_root,
            phase: SnapPhase::Accounts,
            next_account: B256::ZERO,
            next_block: target_block.saturating_add(1),
        }
    }

    /// Pivot and phase represented by this durable download progress.
    pub const fn generation(&self) -> SnapGeneration {
        SnapGeneration::new(BlockNumHash::new(self.target_block, self.target_hash), self.state_root)
            .with_phase(self.phase)
    }

    // Phase checks keep late asynchronous results from crossing durable boundaries.
    pub(crate) fn ensure_phase(&self, expected: SnapPhase) -> Result<(), SnapSyncError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(SnapSyncError::UnexpectedPhase { expected, actual: self.phase })
        }
    }

    // Unknown marker schemas are safer to restart than reinterpret.
    pub(crate) fn validate(&self) -> Result<(), SnapSyncError> {
        if self.version != SNAP_GENERATION_VERSION {
            return Err(SnapSyncError::InvalidGeneration(format!(
                "unsupported version {}",
                self.version
            )));
        }
        let first_bal = self.target_block.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidGeneration("target block has no BAL successor".to_string())
        })?;
        if self.next_block > first_bal {
            return Err(SnapSyncError::InvalidGeneration(
                "BAL cursor does not follow the target".to_string(),
            ));
        }
        Ok(())
    }
}

impl Encodable for SnapPhase {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        (*self as u8).encode(out);
    }

    fn length(&self) -> usize {
        (*self as u8).length()
    }
}

impl Decodable for SnapPhase {
    fn decode(buffer: &mut &[u8]) -> alloy_rlp::Result<Self> {
        match u8::decode(buffer)? {
            0 => Ok(Self::Accounts),
            1 => Ok(Self::BlockAccessLists),
            2 => Ok(Self::Trie),
            _ => Err(alloy_rlp::Error::Custom("unknown snap generation phase")),
        }
    }
}

#[cfg(test)]
impl<F> SnapStateStore<'_, F> {
    // Seeds a durable prefix for lifecycle tests that do not exercise proof verification.
    pub(crate) fn seed_account_state(
        &self,
        generation: SnapDownloadProgress,
        state: reth_trie_common::HashedPostState,
        bytecodes: Vec<(B256, alloy_primitives::Bytes)>,
        next: Option<B256>,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: crate::SnapSyncProvider,
    {
        use reth_storage_api::StateWriter;
        let provider = self.factory.database_provider_rw()?;
        self.ensure_generation(&provider, generation)?;
        let write = provider.active_snap_write()?.ok_or(SnapSyncError::NoAttempt)?;
        assert!(bytecodes.is_empty());
        provider.write_hashed_state(&state.into_sorted())?;
        provider.write_metadata(
            "snap_account_coverage",
            serde_json::to_vec(&serde_json::json!({
                "version": 1, "attempt": write.attempt(), "coverage": { "next": next }
            }))
            .unwrap(),
        )?;
        let generation = Self::load_generation(&provider)?.unwrap();
        provider.commit()?;
        Ok(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
    use reth_primitives_traits::Account;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_storage_api::{StateWriter, StorageSettings};
    use reth_trie_common::HashedPostState;

    fn generation() -> SnapDownloadProgress {
        SnapDownloadProgress::new(100, B256::repeat_byte(1), B256::repeat_byte(2))
    }

    fn account(nonce: u64) -> Account {
        Account { nonce, balance: U256::from(nonce), bytecode_hash: None }
    }

    #[test]
    fn existing_state_cannot_be_replaced_by_a_generation() {
        // Execution can be ahead of Finish after an interrupted ordinary backfill. A completed
        // snapshot also has its own checkpoint, independent of subsequent pipeline progress.
        for stage in [StageId::Execution, StageId::Finish, SNAP_SYNC_STAGE] {
            let factory = crate::test_utils::hashed_factory();
            let provider = factory.database_provider_rw().unwrap();
            provider
                .write_hashed_state(
                    &HashedPostState::default()
                        .with_accounts([(B256::ZERO, Some(account(7)))])
                        .into_sorted(),
                )
                .unwrap();
            provider.save_stage_checkpoint(stage, StageCheckpoint::new(42)).unwrap();
            provider
                .save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::new(42))
                .unwrap();
            provider.commit().unwrap();

            let store = SnapStateStore::new(&factory);
            assert!(!store.requires_bootstrap().unwrap());
            assert!(
                matches!(store.begin_generation(generation()), Err(SnapSyncError::ExistingState)),
                "stage: {stage}"
            );

            let provider = factory.database_provider_ro().unwrap();
            assert_eq!(
                provider.tx_ref().get::<tables::HashedAccounts>(B256::ZERO).unwrap(),
                Some(account(7))
            );
            assert_eq!(
                provider.get_stage_checkpoint(stage).unwrap(),
                Some(StageCheckpoint::new(42))
            );
            assert_eq!(
                provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap(),
                Some(StageCheckpoint::new(42))
            );
            assert!(store.interrupted_generation().unwrap().is_none());
        }
    }

    #[test]
    fn rejects_plain_state_before_clearing_hashed_state() {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        provider
            .write_hashed_state(
                &HashedPostState::default()
                    .with_accounts([(B256::ZERO, Some(account(1)))])
                    .into_sorted(),
            )
            .unwrap();
        provider.commit().unwrap();
        factory.set_storage_settings_cache(StorageSettings::v1());

        let error = SnapStateStore::new(&factory).begin_generation(generation()).unwrap_err();

        assert!(matches!(error, SnapSyncError::UnsupportedStorage));
        let provider = factory.database_provider_ro().unwrap();
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        assert!(cursor.first().unwrap().is_some());
    }

    #[test]
    fn genesis_checkpoints_allow_bootstrap() {
        let factory = crate::test_utils::hashed_factory();
        let provider = factory.database_provider_rw().unwrap();
        for stage in [StageId::Execution, StageId::Finish] {
            provider.save_stage_checkpoint(stage, StageCheckpoint::default()).unwrap();
        }
        provider.commit().unwrap();

        let store = SnapStateStore::new(&factory);
        assert!(store.requires_bootstrap().unwrap());
        store.begin_generation(generation()).unwrap();
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation()));
    }

    #[test]
    fn executed_state_with_an_unfinished_generation_is_rejected() {
        let factory = crate::test_utils::hashed_factory();
        let store = SnapStateStore::new(&factory);
        store.begin_generation(generation()).unwrap();
        let provider = factory.database_provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(1)).unwrap();
        provider.commit().unwrap();

        assert!(matches!(store.requires_bootstrap(), Err(SnapSyncError::ExistingState)));
        assert!(matches!(store.begin_generation(generation()), Err(SnapSyncError::ExistingState)));
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation()));
    }
}
