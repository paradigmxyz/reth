//! Persists verified Snap ranges and their restart cursor atomically.
//!
//! A generation marker remains present until the downloaded state and rebuilt trie are accepted,
//! preventing partial hashed tables from being mistaken for canonical state.

use crate::{error::db_error, SnapSyncError};
use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rlp::{Decodable, Encodable};
use reth_db_api::{tables, transaction::DbTxMut};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::DatabaseProviderFactory;
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    AccountExtReader, DBProvider, HeaderProvider, StageCheckpointReader, StageCheckpointWriter,
    StateWriter, StorageSettingsCache,
};
use reth_trie_common::{
    bal::{deployed_bytecode, hashed_storage_changes, BalAccountState},
    HashedPostState, HashedStorage,
};
use revm::{bytecode::Bytecode, database::states::StateChangeset};

// Existing stage tooling can inspect the generation without a Snap-specific table.
const SNAP_SYNC_STAGE: StageId = StageId::Other("SnapSync");
// Versioning prevents an incompatible restart marker from being reinterpreted.
const SNAP_GENERATION_VERSION: u8 = 1;

/// Owns durable state-generation writes for one provider factory.
#[derive(Debug)]
pub struct SnapStateStore<'a, F> {
    // The store opens one short transaction per durable transition.
    factory: &'a F,
}

impl<'a, F> SnapStateStore<'a, F> {
    /// Creates a store over `factory`.
    pub const fn new(factory: &'a F) -> Self {
        Self { factory }
    }

    /// Starts a clean generation after verifying the canonical state layout.
    pub fn begin_generation(&self, generation: SnapGeneration) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider<Tx: DbTxMut> + StageCheckpointWriter + StorageSettingsCache,
    {
        generation.validate()?;
        generation.ensure_phase(SnapPhase::Accounts)?;
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorageLayout)
        }

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

    /// Commits one verified range and advances its account cursor in the same transaction.
    pub fn commit_account_range(
        &self,
        generation: SnapGeneration,
        state: HashedPostState,
        bytecodes: Vec<(B256, Bytes)>,
        progress: AccountRangeProgress,
    ) -> Result<SnapGeneration, SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        generation.validate()?;
        generation.ensure_phase(SnapPhase::Accounts)?;
        let next_generation = generation.with_account_progress(progress)?;
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorageLayout)
        }
        if Self::load_generation(&provider)? != Some(generation) {
            return Err(SnapSyncError::StaleGeneration)
        }

        if !state.is_empty() {
            provider.write_hashed_state(&state.into_sorted()).map_err(db_error)?;
        }
        let contracts = bytecodes
            .into_iter()
            .filter(|(_, code)| !code.is_empty())
            .map(|(hash, code)| (hash, Bytecode::new_raw(code)))
            .collect::<Vec<_>>();
        if !contracts.is_empty() {
            provider
                .write_state_changes(StateChangeset { contracts, ..Default::default() })
                .map_err(db_error)?;
        }

        Self::save_generation(&provider, next_generation)?;
        provider.commit().map_err(db_error)?;
        Ok(next_generation)
    }

    /// Applies one authenticated BAL and advances its block cursor in the same transaction.
    pub fn commit_block_access_list(
        &self,
        generation: SnapGeneration,
        block_number: u64,
        block_hash: B256,
        block_access_list: &DecodedBal,
        progress: BlockAccessListProgress,
    ) -> Result<SnapGeneration, SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: AccountExtReader
            + DBProvider
            + HeaderProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        generation.validate()?;
        generation.ensure_phase(SnapPhase::BlockAccessLists)?;
        let mut next_generation =
            generation.with_block_access_list_progress(block_number, progress)?;
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorageLayout)
        }
        if Self::load_generation(&provider)? != Some(generation) {
            return Err(SnapSyncError::StaleGeneration)
        }
        Self::canonical_header_fields(&provider, generation.target_block, generation.target_hash)?;
        let (state_root, commitment) =
            Self::canonical_header_fields(&provider, block_number, block_hash)?;
        let commitment = commitment.ok_or_else(|| {
            SnapSyncError::InvalidRequest(format!(
                "canonical header {block_number} has no block access list commitment"
            ))
        })?;
        block_access_list
            .ensure_hash(commitment)
            .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
        if progress == BlockAccessListProgress::Complete {
            next_generation.target_block = block_number;
            next_generation.target_hash = block_hash;
            next_generation.state_root = state_root;
        }
        Self::apply_block_access_list(&provider, block_access_list)?;
        Self::save_generation(&provider, next_generation)?;
        provider.commit().map_err(db_error)?;
        Ok(next_generation)
    }

    // Enters trie generation when the snapshot pivot already matches the catch-up target.
    pub(crate) fn complete_block_access_lists(
        &self,
        generation: SnapGeneration,
    ) -> Result<SnapGeneration, SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider
            + HeaderProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StorageSettingsCache,
    {
        generation.validate()?;
        generation.ensure_phase(SnapPhase::BlockAccessLists)?;
        let next_generation = generation.with_completed_block_access_lists();
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorageLayout)
        }
        if Self::load_generation(&provider)? != Some(generation) {
            return Err(SnapSyncError::StaleGeneration)
        }
        Self::canonical_header_fields(&provider, generation.target_block, generation.target_hash)?;
        Self::save_generation(&provider, next_generation)?;
        provider.commit().map_err(db_error)?;
        Ok(next_generation)
    }

    // Reads parent accounts in one cursor pass before assembling hashed BAL deltas.
    fn apply_block_access_list(
        provider: &(impl AccountExtReader + StateWriter),
        block_access_list: &DecodedBal,
    ) -> Result<(), SnapSyncError> {
        let accounts = provider
            .basic_accounts(block_access_list.as_bal().iter().map(|changes| changes.address))
            .map_err(db_error)?;
        let mut state = HashedPostState::with_capacity(accounts.len());
        let mut contracts = Vec::new();

        for (changes, (_, existing)) in block_access_list.as_bal().iter().zip(accounts) {
            let hashed_address = keccak256(changes.address);
            let account_fields = BalAccountState::from_changes(changes);
            if !account_fields.is_empty() {
                let account = account_fields.merge_onto(existing.as_ref());
                state.accounts.insert(hashed_address, (!account.is_empty()).then_some(account));
            }
            if !changes.storage_changes.is_empty() {
                let mut storage = HashedStorage::new(false);
                storage.storage.extend(hashed_storage_changes(changes));
                state.storages.insert(hashed_address, storage);
            }
            if let Some((hash, code)) = deployed_bytecode(changes) {
                contracts.push((hash, Bytecode::new_raw(code.clone())));
            }
        }

        if !state.is_empty() {
            provider.write_hashed_state(&state.into_sorted()).map_err(db_error)?;
        }
        if !contracts.is_empty() {
            provider
                .write_state_changes(StateChangeset { contracts, ..Default::default() })
                .map_err(db_error)?;
        }
        Ok(())
    }

    // Canonical checks keep a reorged pivot or in-flight BAL outside the durable generation.
    fn canonical_header_fields(
        provider: &impl HeaderProvider,
        block_number: u64,
        expected: B256,
    ) -> Result<(B256, Option<B256>), SnapSyncError> {
        let Some(header) = provider.sealed_header(block_number).map_err(db_error)? else {
            return Err(SnapSyncError::CanonicalHeaderMismatch {
                block_number,
                expected,
                actual: None,
            })
        };
        let actual = header.hash();
        if actual != expected {
            return Err(SnapSyncError::CanonicalHeaderMismatch {
                block_number,
                expected,
                actual: Some(actual),
            })
        }
        Ok((header.state_root(), header.block_access_list_hash()))
    }

    // Stage progress keeps restart data visible to existing database tooling.
    fn save_generation(
        provider: &impl StageCheckpointWriter,
        generation: SnapGeneration,
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
        provider: &impl StageCheckpointReader,
    ) -> Result<Option<SnapGeneration>, SnapSyncError> {
        let Some(encoded) = provider
            .get_stage_checkpoint_progress(SNAP_SYNC_STAGE)
            .map_err(db_error)?
            .filter(|encoded| !encoded.is_empty())
        else {
            return Ok(None)
        };
        let generation = alloy_rlp::decode_exact::<SnapGeneration>(&encoded)
            .map_err(|error| SnapSyncError::InvalidGeneration(error.to_string()))?;
        generation.validate()?;
        Ok(Some(generation))
    }
}

impl<F> SnapStateStore<'_, F>
where
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader,
{
    /// Returns the partial generation that must be resumed before state is served.
    pub fn interrupted_generation(&self) -> Result<Option<SnapGeneration>, SnapSyncError> {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        Self::load_generation(&provider)
    }
}

/// Durable identity and restart position of a Snap state generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
pub struct SnapGeneration {
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

impl SnapGeneration {
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

    // Phase checks keep late asynchronous results from crossing durable boundaries.
    fn ensure_phase(&self, expected: SnapPhase) -> Result<(), SnapSyncError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(SnapSyncError::UnexpectedPhase { expected, actual: self.phase })
        }
    }

    // A monotonic cursor makes every committed prefix safe to resume.
    fn with_account_progress(
        mut self,
        progress: AccountRangeProgress,
    ) -> Result<Self, SnapSyncError> {
        match progress {
            AccountRangeProgress::More { next_account } if next_account > self.next_account => {
                self.next_account = next_account;
            }
            AccountRangeProgress::More { next_account } => {
                return Err(SnapSyncError::NonAdvancingAccountCursor {
                    current: self.next_account,
                    next: next_account,
                })
            }
            AccountRangeProgress::Complete => self.phase = SnapPhase::BlockAccessLists,
        }
        Ok(self)
    }

    // A per-block cursor makes BAL application idempotent across restarts.
    fn with_block_access_list_progress(
        mut self,
        block_number: u64,
        progress: BlockAccessListProgress,
    ) -> Result<Self, SnapSyncError> {
        if block_number != self.next_block {
            return Err(SnapSyncError::UnexpectedBlock {
                expected: self.next_block,
                actual: block_number,
            })
        }
        self.next_block = block_number.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidGeneration(
                "BAL cursor exceeds the block number space".to_string(),
            )
        })?;
        if progress == BlockAccessListProgress::Complete {
            self.phase = SnapPhase::Trie;
        }
        Ok(self)
    }

    // No-op catch-up still needs an explicit durable phase transition.
    const fn with_completed_block_access_lists(mut self) -> Self {
        self.phase = SnapPhase::Trie;
        self
    }

    // Unknown marker schemas are safer to restart than reinterpret.
    fn validate(&self) -> Result<(), SnapSyncError> {
        if self.version != SNAP_GENERATION_VERSION {
            return Err(SnapSyncError::InvalidGeneration(format!(
                "unsupported version {}",
                self.version
            )))
        }
        let first_bal = self.target_block.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidGeneration("target block has no BAL successor".to_string())
        })?;
        if self.next_block < first_bal {
            return Err(SnapSyncError::InvalidGeneration(
                "BAL cursor precedes the target".to_string(),
            ))
        }
        Ok(())
    }
}

/// Restart position after committing an authenticated account range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountRangeProgress {
    /// Another request starts at this inclusive account hash.
    More {
        /// Inclusive origin of the next range.
        next_account: B256,
    },
    /// Account, storage, and bytecode download is complete.
    Complete,
}

/// Restart position after applying an authenticated block access list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAccessListProgress {
    /// More canonical blocks remain before trie generation.
    More,
    /// The catch-up target was applied.
    Complete,
}

/// Durable phase of state assembly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SnapPhase {
    /// Account, storage, and bytecode ranges are being downloaded.
    Accounts = 0,
    /// Authenticated block access lists are being applied.
    BlockAccessLists = 1,
    /// The final state trie is being rebuilt and checked.
    Trie = 2,
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
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
    use reth_primitives_traits::Account;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_storage_api::StorageSettings;

    fn generation() -> SnapGeneration {
        SnapGeneration::new(100, B256::repeat_byte(1), B256::repeat_byte(2))
    }

    fn account(nonce: u64) -> Account {
        Account { nonce, balance: U256::from(nonce), bytecode_hash: None }
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

        assert!(matches!(error, SnapSyncError::UnsupportedStorageLayout));
        let provider = factory.database_provider_ro().unwrap();
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        assert!(cursor.first().unwrap().is_some());
    }

    #[test]
    fn account_range_and_cursor_commit_together() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let store = SnapStateStore::new(&factory);
        let generation = generation();
        store.begin_generation(generation).unwrap();
        let account_hash = B256::repeat_byte(3);
        let code = Bytes::from_static(&[0x60, 0x00]);
        let code_hash = alloy_primitives::keccak256(&code);
        let state = HashedPostState::default().with_accounts([(
            account_hash,
            Some(Account { bytecode_hash: Some(code_hash), ..account(7) }),
        )]);

        let updated = store
            .commit_account_range(
                generation,
                state,
                vec![(code_hash, code.clone())],
                AccountRangeProgress::More { next_account: account_hash },
            )
            .unwrap();

        assert_eq!(store.interrupted_generation().unwrap(), Some(updated));
        assert_eq!(updated.next_account, account_hash);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider
                .tx_ref()
                .get::<tables::Bytecodes>(code_hash)
                .unwrap()
                .unwrap()
                .original_bytes(),
            code
        );
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        assert_eq!(cursor.seek_exact(account_hash).unwrap().unwrap().1.nonce, 7);
    }

    #[test]
    fn final_account_range_sets_bal_cursor() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let store = SnapStateStore::new(&factory);
        let generation = generation();
        store.begin_generation(generation).unwrap();

        let updated = store
            .commit_account_range(
                generation,
                HashedPostState::default(),
                Vec::new(),
                AccountRangeProgress::Complete,
            )
            .unwrap();

        assert_eq!(updated.phase, SnapPhase::BlockAccessLists);
        assert_eq!(updated.next_block, generation.target_block + 1);
        assert_eq!(store.interrupted_generation().unwrap(), Some(updated));
    }

    #[test]
    fn stale_range_cannot_advance_generation() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let store = SnapStateStore::new(&factory);
        let generation = generation();
        store.begin_generation(generation).unwrap();
        let updated = store
            .commit_account_range(
                generation,
                HashedPostState::default(),
                Vec::new(),
                AccountRangeProgress::More { next_account: B256::repeat_byte(1) },
            )
            .unwrap();

        let error = store
            .commit_account_range(
                generation,
                HashedPostState::default(),
                Vec::new(),
                AccountRangeProgress::Complete,
            )
            .unwrap_err();

        assert!(matches!(error, SnapSyncError::StaleGeneration));
        assert_eq!(store.interrupted_generation().unwrap(), Some(updated));
    }

    #[test]
    fn continuation_must_advance_account_cursor() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let store = SnapStateStore::new(&factory);
        let generation = generation();
        store.begin_generation(generation).unwrap();

        let error = store
            .commit_account_range(
                generation,
                HashedPostState::default(),
                Vec::new(),
                AccountRangeProgress::More { next_account: B256::ZERO },
            )
            .unwrap_err();

        assert!(matches!(error, SnapSyncError::NonAdvancingAccountCursor { .. }));
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
    }
}
