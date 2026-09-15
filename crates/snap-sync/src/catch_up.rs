//! Advances downloaded flat state with authenticated EIP-7928 block access lists.
//!
//! Only a contiguous prefix of locally canonical headers is applied, and each block update is
//! committed with its restart cursor before the next network response is considered.

use crate::{
    bytecode::store::write_bytecodes,
    common::{push_peer, request_options, SnapRequests},
    error::db_error,
    SnapDownloadProgress, SnapPhase, SnapStateStore, SnapSyncError,
};
use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{keccak256, B256};
use reth_downloaders::snap::{BlockAccessListDownloader, BlockAccessListOutcome};
use reth_eth_wire_types::snap::GetBlockAccessListsMessage;
use reth_network_p2p::{error::RequestError, snap::client::SnapClient};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{
    AccountExtReader, DBProvider, HeaderProvider, StageCheckpointReader, StageCheckpointWriter,
    StateWriter, StorageSettingsCache,
};
use reth_tasks::Runtime;
use reth_trie_common::{
    bal::{deployed_bytecode, hashed_storage_changes, BalAccountState},
    HashedPostState, HashedStorage,
};
use revm::bytecode::Bytecode;

// EIP-8189 recommends a 2 MiB BAL response to limit memory and round-trip overhead.
const BAL_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
// Twenty-eight average 60M-gas BALs fit below the recommended response size.
const BAL_BLOCKS_PER_REQUEST: u64 = 28;

/// Applies authenticated BALs from the downloaded pivot through a canonical target block.
#[derive(Debug)]
pub struct BlockAccessListCatchUp<'a, C, F> {
    // Client, runtime and request ids shared with the state downloads.
    requests: SnapRequests<'a, C>,
    // Header reads and state transitions must observe the same provider factory.
    factory: &'a F,
    // Every applied block advances the store's durable generation marker.
    store: SnapStateStore<'a, F>,
}

impl<'a, C, F> BlockAccessListCatchUp<'a, C, F> {
    /// Creates a catch-up coordinator without starting network or database work.
    pub const fn new(client: &'a C, factory: &'a F, runtime: Runtime) -> Self {
        Self {
            requests: SnapRequests::new(client, runtime),
            factory,
            store: SnapStateStore::new(factory),
        }
    }

    /// Applies the canonical BAL prefix or returns when every eligible peer lacks the next BAL.
    pub async fn run(
        &mut self,
        generation: SnapDownloadProgress,
        target_block: u64,
    ) -> Result<BlockAccessListCatchUpOutcome, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory,
        F::Provider: HeaderProvider,
        F::ProviderRW: AccountExtReader
            + DBProvider
            + HeaderProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        match self.run_inner(generation, target_block, CatchUpMode::FullState).await? {
            CatchUpAttempt::Complete(generation) => {
                Ok(BlockAccessListCatchUpOutcome::Complete { generation })
            }
            CatchUpAttempt::Unavailable(generation) => {
                Ok(BlockAccessListCatchUpOutcome::Unavailable { generation })
            }
        }
    }

    /// Moves a partial account prefix without touching its pending suffix.
    pub async fn advance_pivot(
        &mut self,
        generation: SnapDownloadProgress,
        target_block: u64,
    ) -> Result<BlockAccessListCatchUpOutcome, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory,
        F::Provider: HeaderProvider,
        F::ProviderRW: AccountExtReader
            + DBProvider
            + HeaderProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        match self.run_inner(generation, target_block, CatchUpMode::PartialPivot).await? {
            CatchUpAttempt::Complete(generation) => {
                Ok(BlockAccessListCatchUpOutcome::Complete { generation })
            }
            CatchUpAttempt::Unavailable(generation) => {
                Ok(BlockAccessListCatchUpOutcome::Unavailable { generation })
            }
        }
    }

    // Shares bounded BAL retrieval while preserving each generation phase's write scope.
    async fn run_inner(
        &mut self,
        mut generation: SnapDownloadProgress,
        target_block: u64,
        mode: CatchUpMode,
    ) -> Result<CatchUpAttempt, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory,
        F::Provider: HeaderProvider,
        F::ProviderRW: AccountExtReader
            + DBProvider
            + HeaderProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        if generation.phase != mode.phase() {
            return Err(SnapSyncError::UnexpectedPhase {
                expected: mode.phase(),
                actual: generation.phase,
            })
        }
        let applied_through = generation.next_block.checked_sub(1).ok_or_else(|| {
            SnapSyncError::InvalidGeneration("BAL cursor starts before block zero".to_string())
        })?;
        if target_block < applied_through {
            return Err(SnapSyncError::InvalidRequest(format!(
                "BAL target {target_block} precedes applied block {applied_through}"
            )))
        }
        if target_block == applied_through {
            if mode == CatchUpMode::FullState {
                generation = self.store.complete_block_access_lists(generation)?;
            }
            return Ok(CatchUpAttempt::Complete(generation))
        }

        let mut excluded = Vec::new();
        loop {
            let headers = self.canonical_headers(generation, target_block)?;
            let request = GetBlockAccessListsMessage {
                request_id: self.requests.next_id()?,
                block_hashes: headers.iter().map(|header| header.hash()).collect(),
                response_bytes: BAL_RESPONSE_BYTES,
            };
            let downloader = BlockAccessListDownloader::new_with_options(
                self.requests.client,
                request,
                &headers,
                self.requests.runtime.clone(),
                request_options(&excluded),
            )
            .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
            match downloader.await {
                Ok(BlockAccessListOutcome::Unavailable { peer_id }) => {
                    push_peer(&mut excluded, peer_id)
                }
                Err(RequestError::UnsupportedCapability) => {
                    return Ok(CatchUpAttempt::Unavailable(generation))
                }
                Err(error) => return Err(error.into()),
                Ok(BlockAccessListOutcome::Verified(verified)) => {
                    let unavailable =
                        (!verified.missing().is_empty()).then_some(verified.peer_id());
                    for (header, (_, block_access_list)) in
                        headers.iter().zip(verified.into_block_access_lists())
                    {
                        let Some(block_access_list) = block_access_list else { break };
                        let complete = header.number() == target_block;
                        generation = match mode {
                            CatchUpMode::FullState => {
                                let progress = if complete {
                                    BlockAccessListProgress::Complete
                                } else {
                                    BlockAccessListProgress::More
                                };
                                self.store.commit_block_access_list(
                                    generation,
                                    header.number(),
                                    header.hash(),
                                    &block_access_list,
                                    progress,
                                )?
                            }
                            CatchUpMode::PartialPivot => {
                                self.store.commit_pivot_block_access_list(
                                    generation,
                                    header.number(),
                                    header.hash(),
                                    &block_access_list,
                                )?
                            }
                        };
                        if complete {
                            return Ok(CatchUpAttempt::Complete(generation))
                        }
                    }
                    if let Some(peer_id) = unavailable {
                        push_peer(&mut excluded, peer_id);
                    } else {
                        excluded.clear();
                    }
                }
            }
        }
    }

    // Reads one bounded canonical header batch and rejects a reorged generation anchor early.
    fn canonical_headers(
        &self,
        generation: SnapDownloadProgress,
        target_block: u64,
    ) -> Result<
        Vec<reth_primitives_traits::SealedHeader<<F::Provider as HeaderProvider>::Header>>,
        SnapSyncError,
    >
    where
        F: DatabaseProviderFactory,
        F::Provider: HeaderProvider,
    {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        let anchor = provider
            .sealed_header(generation.target_block)
            .map_err(db_error)?
            .ok_or(SnapSyncError::MissingHeader(generation.target_block))?;
        if anchor.hash() != generation.target_hash {
            return Err(SnapSyncError::CanonicalHeaderMismatch {
                block_number: generation.target_block,
                expected: generation.target_hash,
                actual: Some(anchor.hash()),
            })
        }

        let end =
            generation.next_block.saturating_add(BAL_BLOCKS_PER_REQUEST - 1).min(target_block);
        let headers =
            provider.sealed_headers_range(generation.next_block..=end).map_err(db_error)?;
        for (number, header) in (generation.next_block..=end).zip(&headers) {
            if header.number() != number {
                return Err(SnapSyncError::MissingHeader(number))
            }
        }
        let expected = usize::try_from(end - generation.next_block + 1)
            .expect("BAL header batches contain at most 28 entries");
        if headers.len() != expected {
            return Err(SnapSyncError::MissingHeader(generation.next_block + headers.len() as u64))
        }
        Ok(headers)
    }
}

/// Terminal result of one BAL catch-up attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAccessListCatchUpOutcome {
    /// Every block through the target was applied.
    Complete {
        /// Updated durable generation.
        generation: SnapDownloadProgress,
    },
    /// No eligible peer currently serves the first unapplied block.
    Unavailable {
        /// Last fully applied durable generation position.
        generation: SnapDownloadProgress,
    },
}

// Distinguishes full-state catch-up from partial-prefix pivot advancement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatchUpMode {
    // Completes downloaded state.
    FullState,
    // Advances only the downloaded prefix.
    PartialPivot,
}

impl CatchUpMode {
    // Each mode is valid only while its corresponding generation phase is active.
    const fn phase(self) -> SnapPhase {
        match self {
            Self::FullState => SnapPhase::BlockAccessLists,
            Self::PartialPivot => SnapPhase::Accounts,
        }
    }
}

// Keeps the shared retrieval loop independent of its public outcome type.
enum CatchUpAttempt {
    // Reached the requested block.
    Complete(SnapDownloadProgress),
    // Stopped before an unavailable block.
    Unavailable(SnapDownloadProgress),
}

/// Restart position after applying an authenticated block access list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAccessListProgress {
    /// More canonical blocks remain before trie generation.
    More,
    /// The catch-up target was applied.
    Complete,
}

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
            let account_fields = BalAccountState::from_changes(changes);
            if !account_fields.is_empty() {
                let account = account_fields.merge_onto(existing.as_ref());
                state.accounts.insert(hashed_address, (!account.is_empty()).then_some(account));
            }
            if !changes.storage_changes.is_empty() {
                let mut storage = HashedStorage::default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AccountRangeProgress;
    use alloy_consensus::Header;
    use alloy_eip7928::{
        bal::{Bal, DecodedBal},
        AccountChanges, BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges,
        StorageChange,
    };
    use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
    use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
    use reth_downloaders::snap::test_utils::TestSnapClient;
    use reth_eth_wire_types::{snap::BlockAccessListsMessage, BlockAccessLists};
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::create_test_provider_factory, StaticFileProviderFactory, StaticFileWriter,
    };
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::StorageSettings;
    use reth_trie_common::HashedPostState;

    fn decoded_bal(changes: AccountChanges) -> (DecodedBal, Bytes) {
        let raw = Bytes::from(alloy_rlp::encode(Bal::new(vec![changes])));
        (DecodedBal::from_rlp_bytes(raw.clone()).unwrap(), raw)
    }

    fn response(
        peer_id: PeerId,
        request_id: u64,
        entries: Vec<Option<Bytes>>,
    ) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            peer_id,
            SnapResponse::BlockAccessLists(BlockAccessListsMessage {
                request_id,
                block_access_lists: BlockAccessLists(entries),
            }),
        ))
    }

    #[tokio::test]
    async fn applies_contiguous_bals_across_unavailable_peer() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let address = Address::repeat_byte(0x11);
        let slot = U256::from(7);
        let code = Bytes::from_static(&[0x60, 0x00]);
        let index = BlockAccessIndex::new(1);
        let first_changes = AccountChanges::new(address)
            .with_balance_change(BalanceChange::new(index, U256::from(9)))
            .with_nonce_change(NonceChange::new(index, 3))
            .with_code_change(CodeChange::new(index, code.clone()))
            .with_storage_change(SlotChanges::new(
                slot,
                vec![StorageChange::new(index, U256::from(11))],
            ));
        let second_changes = AccountChanges::new(address)
            .with_balance_change(BalanceChange::new(index, U256::from(10)));
        let (first_bal, first_raw) = decoded_bal(first_changes);
        let (second_bal, second_raw) = decoded_bal(second_changes);
        let header0 = Header { number: 0, ..Default::default() };
        let hash0 = header0.hash_slow();
        let header1 = Header {
            number: 1,
            parent_hash: hash0,
            block_access_list_hash: Some(first_bal.hash()),
            ..Default::default()
        };
        let hash1 = header1.hash_slow();
        let header2 = Header {
            number: 2,
            parent_hash: hash1,
            block_access_list_hash: Some(second_bal.hash()),
            ..Default::default()
        };
        let hash2 = header2.hash_slow();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.append_header(&header0, &hash0).unwrap();
        writer.append_header(&header1, &hash1).unwrap();
        writer.append_header(&header2, &hash2).unwrap();
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);

        let store = SnapStateStore::new(&factory);
        let generation = SnapDownloadProgress::new(0, hash0, B256::repeat_byte(1));
        store.begin_generation(generation).unwrap();
        let hashed_address = keccak256(address);
        let generation = store
            .commit_account_range(
                generation,
                HashedPostState::default().with_accounts([(
                    hashed_address,
                    Some(Account { balance: U256::from(1), ..Default::default() }),
                )]),
                Vec::new(),
                AccountRangeProgress::Complete,
            )
            .unwrap();
        let unavailable = PeerId::random();
        let available = PeerId::random();
        let client = TestSnapClient::new([
            response(unavailable, 1, vec![Some(first_raw), None]),
            response(available, 2, vec![Some(second_raw)]),
        ]);

        let outcome = BlockAccessListCatchUp::new(&client, &factory, Runtime::test())
            .run(generation, 2)
            .await
            .unwrap();

        let BlockAccessListCatchUpOutcome::Complete { generation } = outcome else {
            panic!("completed BAL catch-up")
        };
        assert_eq!(generation.phase, SnapPhase::Trie);
        assert_eq!(generation.next_block, 3);
        assert_eq!(generation.target_block, 2);
        assert_eq!(generation.target_hash, hash2);
        assert_eq!(generation.state_root, header2.state_root);
        assert_eq!(*client.exclusions(), [vec![], vec![unavailable]]);
        let provider = factory.database_provider_ro().unwrap();
        let account =
            provider.tx_ref().get::<tables::HashedAccounts>(hashed_address).unwrap().unwrap();
        assert_eq!(account.balance, U256::from(10));
        assert_eq!(account.nonce, 3);
        assert_eq!(account.bytecode_hash, Some(keccak256(&code)));
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
        let hashed_slot = keccak256(slot.to_be_bytes::<32>());
        assert_eq!(
            cursor.seek_by_key_subkey(hashed_address, hashed_slot).unwrap().unwrap().value,
            U256::from(11)
        );
        assert_eq!(
            provider
                .tx_ref()
                .get::<tables::Bytecodes>(keccak256(&code))
                .unwrap()
                .unwrap()
                .original_bytes(),
            code
        );
    }

    #[tokio::test]
    async fn pivot_advancement_updates_only_downloaded_prefix() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let first = Address::repeat_byte(0x11);
        let second = Address::repeat_byte(0x22);
        let mut addresses = [(keccak256(first), first), (keccak256(second), second)];
        addresses.sort_unstable_by_key(|(hash, _)| *hash);
        let [(low_hash, low_address), (high_hash, high_address)] = addresses;
        let index = BlockAccessIndex::new(1);
        let mut changes = vec![
            AccountChanges::new(low_address)
                .with_balance_change(BalanceChange::new(index, U256::from(9))),
            AccountChanges::new(high_address)
                .with_balance_change(BalanceChange::new(index, U256::from(10))),
        ];
        changes.sort_unstable_by_key(|changes| changes.address);
        let raw = Bytes::from(alloy_rlp::encode(Bal::new(changes)));
        let bal = DecodedBal::from_rlp_bytes(raw.clone()).unwrap();
        let header0 = Header { state_root: B256::repeat_byte(1), ..Default::default() };
        let hash0 = header0.hash_slow();
        let header1 = Header {
            number: 1,
            parent_hash: hash0,
            state_root: B256::repeat_byte(2),
            block_access_list_hash: Some(bal.hash()),
            ..Default::default()
        };
        let hash1 = header1.hash_slow();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.append_header(&header0, &hash0).unwrap();
        writer.append_header(&header1, &hash1).unwrap();
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);

        let store = SnapStateStore::new(&factory);
        let generation = SnapDownloadProgress::new(0, hash0, header0.state_root);
        store.begin_generation(generation).unwrap();
        let generation = store
            .commit_account_range(
                generation,
                HashedPostState::default().with_accounts([(
                    low_hash,
                    Some(Account { balance: U256::from(1), ..Default::default() }),
                )]),
                Vec::new(),
                AccountRangeProgress::More { next_account: high_hash },
            )
            .unwrap();
        let client = TestSnapClient::new([response(PeerId::random(), 1, vec![Some(raw)])]);

        let outcome = BlockAccessListCatchUp::new(&client, &factory, Runtime::test())
            .advance_pivot(generation, 1)
            .await
            .unwrap();

        let BlockAccessListCatchUpOutcome::Complete { generation } = outcome else {
            panic!("completed pivot advancement")
        };
        assert_eq!(generation.phase, SnapPhase::Accounts);
        assert_eq!(generation.target_block, 1);
        assert_eq!(generation.target_hash, hash1);
        assert_eq!(generation.state_root, header1.state_root);
        assert_eq!(generation.next_account, high_hash);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(low_hash).unwrap().unwrap().balance,
            U256::from(9)
        );
        assert!(provider.tx_ref().get::<tables::HashedAccounts>(high_hash).unwrap().is_none());
    }

    #[tokio::test]
    async fn rejects_reorged_pivot_before_requesting_bal() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let header = Header { number: 0, ..Default::default() };
        let actual_hash = header.hash_slow();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.append_header(&header, &actual_hash).unwrap();
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);
        let store = SnapStateStore::new(&factory);
        let generation =
            SnapDownloadProgress::new(0, B256::repeat_byte(0xff), B256::repeat_byte(1));
        store.begin_generation(generation).unwrap();
        let generation = store
            .commit_account_range(
                generation,
                HashedPostState::default(),
                Vec::new(),
                AccountRangeProgress::Complete,
            )
            .unwrap();
        let client = TestSnapClient::new(std::iter::empty());

        let error = BlockAccessListCatchUp::new(&client, &factory, Runtime::test())
            .run(generation, 1)
            .await
            .unwrap_err();

        assert!(matches!(error, SnapSyncError::CanonicalHeaderMismatch { .. }));
        assert!(client.priorities().is_empty());
    }
}
