//! Commits verified block access lists together with the generation cursor.

use super::BlockAccessListProgress;
use crate::{
    error::db_error, state::bytecode::store::write_bytecodes, SnapDownloadProgress, SnapPhase,
    SnapStateStore, SnapSyncError,
};
use alloy_eip7928::{bal::DecodedBal, AccountChanges};
use alloy_primitives::{keccak256, B256};
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{
    AccountExtReader, DBProvider, HeaderProvider, StageCheckpointReader, StageCheckpointWriter,
    StateWriter, StorageSettingsCache,
};
use reth_trie_common::{HashedPostState, HashedStorage};
use revm::bytecode::Bytecode;
use std::borrow::Cow;

impl<F> SnapStateStore<'_, F> {
    /// Applies one authenticated BAL and advances its block cursor in the same transaction.
    pub fn commit_block_access_list(
        &self,
        generation: SnapDownloadProgress,
        block_number: u64,
        block_hash: B256,
        block_access_list: &DecodedBal,
        progress: BlockAccessListProgress,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
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
        self.commit_verified_block_access_list(
            generation,
            block_number,
            block_hash,
            block_access_list,
            BlockAccessListCommit::CatchUp(progress),
        )
    }

    // Advances a partial account prefix without writing state at or beyond its restart cursor.
    pub(crate) fn commit_pivot_block_access_list(
        &self,
        generation: SnapDownloadProgress,
        block_number: u64,
        block_hash: B256,
        block_access_list: &DecodedBal,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
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
        self.commit_verified_block_access_list(
            generation,
            block_number,
            block_hash,
            block_access_list,
            BlockAccessListCommit::Pivot,
        )
    }

    // Enters trie generation when the snapshot pivot already matches the catch-up target.
    pub(crate) fn complete_block_access_lists(
        &self,
        generation: SnapDownloadProgress,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
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
        self.ensure_generation(&provider, generation)?;
        Self::canonical_header_fields(&provider, generation.target_block, generation.target_hash)?;
        Self::save_generation(&provider, next_generation)?;
        provider.commit().map_err(db_error)?;
        Ok(next_generation)
    }

    // Verifies and applies one BAL under the active generation transaction.
    fn commit_verified_block_access_list(
        &self,
        generation: SnapDownloadProgress,
        block_number: u64,
        block_hash: B256,
        block_access_list: &DecodedBal,
        commit: BlockAccessListCommit,
    ) -> Result<SnapDownloadProgress, SnapSyncError>
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
        generation.ensure_phase(commit.phase())?;
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        if !provider.cached_storage_settings().use_hashed_state() {
            return Err(SnapSyncError::UnsupportedStorageLayout)
        }
        self.ensure_generation(&provider, generation)?;
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
        let next_generation = generation.with_applied_block_access_list(
            block_number,
            block_hash,
            state_root,
            commit.completes_generation(),
        )?;
        let account_limit = commit.account_limit(generation.next_account);
        Self::apply_block_access_list(&provider, block_access_list, account_limit)?;
        Self::save_generation(&provider, next_generation)?;
        provider.commit().map_err(db_error)?;
        Ok(next_generation)
    }

    // Reads parent accounts in one cursor pass before assembling hashed BAL deltas.
    fn apply_block_access_list(
        provider: &(impl AccountExtReader + StateWriter),
        block_access_list: &DecodedBal,
        account_limit: Option<B256>,
    ) -> Result<(), SnapSyncError> {
        let changes = block_access_list
            .as_bal()
            .iter()
            .filter_map(|changes| {
                let hashed_address = keccak256(changes.address);
                account_limit
                    .is_none_or(|limit| hashed_address < limit)
                    .then_some((changes, hashed_address))
            })
            .collect::<Vec<_>>();
        let accounts = provider
            .basic_accounts(changes.iter().map(|(changes, _)| changes.address))
            .map_err(db_error)?;
        let mut state = HashedPostState::with_capacity(accounts.len());
        let mut contracts = Vec::new();

        for ((changes, hashed_address), (_, existing)) in changes.into_iter().zip(accounts) {
            let changes = ordered_changes(changes);
            let account_fields = changes.account_info();
            if !account_fields.is_empty() {
                let mut account = existing.unwrap_or_default();
                account.apply_bal_info(account_fields);
                state.accounts.insert(hashed_address, (!account.is_empty()).then_some(account));
            }
            if !changes.storage_changes.is_empty() {
                let mut storage = HashedStorage::default();
                storage.storage.extend(
                    changes
                        .storage_post_states()
                        .map(|(slot, value)| (keccak256(B256::from(slot)), value)),
                );
                state.storages.insert(hashed_address, storage);
            }
            if let Some((hash, code)) = account_fields
                .code_hash
                .zip(changes.code_post_state().filter(|code| !code.is_empty()))
            {
                contracts.push((hash, Bytecode::new_raw(code.clone())));
            }
        }

        if !state.is_empty() {
            provider.write_hashed_state(&state.into_sorted()).map_err(db_error)?;
        }
        if !contracts.is_empty() {
            write_bytecodes(provider, contracts).map_err(db_error)?;
        }
        Ok(())
    }
}

// Selects whether a BAL advances a partial pivot or the complete downloaded state.
#[derive(Clone, Copy, Debug)]
enum BlockAccessListCommit {
    // Applies changes to the complete downloaded state.
    CatchUp(BlockAccessListProgress),
    // Applies changes only below the partial account cursor.
    Pivot,
}

impl BlockAccessListCommit {
    // Partial pivots remain in account download while full state enters BAL catch-up.
    const fn phase(self) -> SnapPhase {
        match self {
            Self::CatchUp(_) => SnapPhase::BlockAccessLists,
            Self::Pivot => SnapPhase::Accounts,
        }
    }

    // Only the final full-state BAL transitions into trie generation.
    const fn completes_generation(self) -> bool {
        matches!(self, Self::CatchUp(BlockAccessListProgress::Complete))
    }

    // Partial pivots may update only hashes strictly below the persisted account origin.
    const fn account_limit(self, next_account: B256) -> Option<B256> {
        match self {
            Self::CatchUp(_) => None,
            Self::Pivot => Some(next_account),
        }
    }
}

// alloy reads the last change; normalize unordered entries without cloning canonical ones.
fn ordered_changes(changes: &AccountChanges) -> Cow<'_, AccountChanges> {
    if changes.balance_changes.is_sorted_by_key(|change| change.block_access_index) &&
        changes.nonce_changes.is_sorted_by_key(|change| change.block_access_index) &&
        changes.code_changes.is_sorted_by_key(|change| change.block_access_index) &&
        changes
            .storage_changes
            .iter()
            .all(|slot| slot.changes.is_sorted_by_key(|change| change.block_access_index))
    {
        return Cow::Borrowed(changes)
    }
    let mut ordered = changes.clone();
    ordered.sort();
    Cow::Owned(ordered)
}
