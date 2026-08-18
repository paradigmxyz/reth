//! Retrieves authenticated account state and its storage and bytecode dependencies.
//!
//! Complete account batches are committed with their restart cursor so an interruption never
//! exposes a durable range whose dependent state is still missing.

use crate::{AccountRangeProgress, SnapGeneration, SnapPhase, SnapStateStore, SnapSyncError};
use alloy_primitives::{map::B256Set, Bytes, B256, KECCAK256_EMPTY};
use reth_downloaders::snap::{
    AccountRangeDownloader, AccountRangeOutcome, BytecodeDownloader, BytecodeOutcome,
    StorageRangeContinuation, StorageRangeDownloader, StorageRangeOutcome, VerifiedAccountRange,
};
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage,
};
use reth_network_p2p::{
    error::RequestError,
    priority::Priority,
    snap::client::{SnapClient, SnapRequestOptions},
};
use reth_network_peers::PeerId;
use reth_primitives_traits::Account;
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{
    DBProvider, StageCheckpointReader, StageCheckpointWriter, StateWriter, StorageSettingsCache,
};
use reth_tasks::Runtime;
use reth_trie_common::{HashedPostState, HashedStorage, TrieAccount, EMPTY_ROOT_HASH};

// Keeps account and storage requests inclusive through the full trie keyspace.
const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);
// Matches the common serving cap without inviting systematic response truncation.
const STATE_RESPONSE_BYTES: u64 = 512 * 1024;
// A 1 KiB estimate prevents small storage tries from overfilling a response request.
const STATE_ACCOUNTS_PER_BATCH: usize = STATE_RESPONSE_BYTES as usize / 1024;
// Four average-sized requests per maximum EVM bytecode balances gaps against truncation.
const BYTECODE_HASHES_PER_REQUEST: usize = STATE_RESPONSE_BYTES as usize / (24 * 1024) * 4;

/// Downloads and durably assembles the flat state authenticated by one generation root.
#[derive(Debug)]
pub struct StateDownloader<'a, C, F> {
    // A reference avoids requiring network clients to implement Clone.
    client: &'a C,
    // All state and cursor transitions share the same provider factory.
    store: SnapStateStore<'a, F>,
    // Proof verification must stay off the async worker.
    runtime: Runtime,
    // Request IDs remain unique for this downloader instance.
    request_id: u64,
}

impl<'a, C, F> StateDownloader<'a, C, F> {
    /// Creates a state downloader without starting network or database work.
    pub const fn new(client: &'a C, factory: &'a F, runtime: Runtime) -> Self {
        Self { client, store: SnapStateStore::new(factory), runtime, request_id: 0 }
    }

    /// Resumes account state download until it completes or every eligible peer is unavailable.
    pub async fn run(
        &mut self,
        mut generation: SnapGeneration,
    ) -> Result<StateDownloadOutcome, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        if generation.phase != SnapPhase::Accounts {
            return Err(SnapSyncError::UnexpectedPhase {
                expected: SnapPhase::Accounts,
                actual: generation.phase,
            })
        }

        loop {
            let Some(range) = self.download_account_range(generation).await? else {
                return Ok(StateDownloadOutcome::Unavailable { generation })
            };
            if range.accounts.is_empty() {
                if range.has_more {
                    return Err(SnapSyncError::InvalidRequest(
                        "account range requires continuation without advancing".to_string(),
                    ))
                }
                generation = self.store.commit_account_range(
                    generation,
                    HashedPostState::default(),
                    Vec::new(),
                    AccountRangeProgress::Complete,
                )?;
                return Ok(StateDownloadOutcome::Complete { generation })
            }

            let account_count = range.accounts.len();
            for (index, accounts) in range.accounts.chunks(STATE_ACCOUNTS_PER_BATCH).enumerate() {
                let Some(storages) = self.download_storages(generation, accounts).await? else {
                    return Ok(StateDownloadOutcome::Unavailable { generation })
                };
                let Some(bytecodes) = self.download_bytecodes(accounts).await? else {
                    return Ok(StateDownloadOutcome::Unavailable { generation })
                };
                let state = HashedPostState::default()
                    .with_accounts(
                        accounts
                            .iter()
                            .map(|(hash, account)| (*hash, Some(Account::from(*account)))),
                    )
                    .with_storages(storages);
                let committed = (index + 1) * STATE_ACCOUNTS_PER_BATCH;
                let is_last = committed >= account_count;
                let progress = if is_last && !range.has_more {
                    AccountRangeProgress::Complete
                } else {
                    AccountRangeProgress::More {
                        next_account: next_hash(
                            accounts.last().expect("account batch is non-empty").0,
                        )
                        .ok_or_else(|| {
                            SnapSyncError::InvalidRequest(
                                "account range continues past the maximum hash".to_string(),
                            )
                        })?,
                    }
                };
                generation =
                    self.store.commit_account_range(generation, state, bytecodes, progress)?;
            }

            if generation.phase != SnapPhase::Accounts {
                return Ok(StateDownloadOutcome::Complete { generation })
            }
        }
    }

    // Retries unavailable roots across distinct peers without penalizing them.
    async fn download_account_range(
        &mut self,
        generation: SnapGeneration,
    ) -> Result<Option<VerifiedAccountRange>, SnapSyncError>
    where
        C: SnapClient,
    {
        let mut excluded = Vec::new();
        loop {
            let request = GetAccountRangeMessage {
                request_id: self.next_request_id()?,
                root_hash: generation.state_root,
                starting_hash: generation.next_account,
                limit_hash: MAX_HASH,
                response_bytes: STATE_RESPONSE_BYTES,
            };
            let downloader = AccountRangeDownloader::new_with_options(
                self.client,
                request,
                self.runtime.clone(),
                request_options(&excluded),
            )
            .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
            match downloader.await {
                Ok(AccountRangeOutcome::Verified(range)) => return Ok(Some(range)),
                Ok(AccountRangeOutcome::Unavailable { peer_id }) => {
                    push_peer(&mut excluded, peer_id)
                }
                Err(RequestError::UnsupportedCapability) => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        }
    }

    // Completes every non-empty storage trie before its owning accounts become durable.
    async fn download_storages(
        &mut self,
        generation: SnapGeneration,
        accounts: &[(B256, TrieAccount)],
    ) -> Result<Option<Vec<(B256, HashedStorage)>>, SnapSyncError>
    where
        C: SnapClient,
    {
        let storage_accounts = accounts
            .iter()
            .filter(|(_, account)| account.storage_root != EMPTY_ROOT_HASH)
            .copied()
            .collect::<Vec<_>>();
        if storage_accounts.is_empty() {
            return Ok(Some(Vec::new()))
        }

        let mut storages = storage_accounts
            .iter()
            .map(|(hash, _)| (*hash, HashedStorage::new(true)))
            .collect::<Vec<_>>();
        let mut pending = storage_accounts.as_slice();
        let mut origin = B256::ZERO;
        let mut excluded = Vec::new();
        while !pending.is_empty() {
            let request = GetStorageRangesMessage {
                request_id: self.next_request_id()?,
                root_hash: generation.state_root,
                account_hashes: pending.iter().map(|(hash, _)| *hash).collect(),
                starting_hash: origin.into(),
                limit_hash: MAX_HASH.into(),
                response_bytes: STATE_RESPONSE_BYTES,
            };
            let downloader = StorageRangeDownloader::new_with_options(
                self.client,
                request,
                pending,
                self.runtime.clone(),
                request_options(&excluded),
            )
            .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
            match downloader.await {
                Ok(StorageRangeOutcome::Unavailable { peer_id }) => {
                    push_peer(&mut excluded, peer_id)
                }
                Err(RequestError::UnsupportedCapability) => return Ok(None),
                Err(error) => return Err(error.into()),
                Ok(StorageRangeOutcome::Verified(verified)) => {
                    for range in verified.ranges {
                        let storage = storages
                            .iter_mut()
                            .find(|(hash, _)| *hash == range.account_hash)
                            .expect("verified range belongs to a requested account");
                        storage.1.storage.extend(range.slots);
                    }
                    let previous_len = pending.len();
                    let previous_origin = origin;
                    match verified.continuation {
                        Some(StorageRangeContinuation::Partial {
                            account_index,
                            starting_hash,
                            ..
                        }) => {
                            pending = pending.get(account_index..).ok_or_else(|| {
                                SnapSyncError::InvalidRequest(
                                    "storage continuation exceeds the request".to_string(),
                                )
                            })?;
                            origin = starting_hash;
                        }
                        Some(StorageRangeContinuation::NextAccount { account_index, .. }) => {
                            pending = pending.get(account_index..).ok_or_else(|| {
                                SnapSyncError::InvalidRequest(
                                    "storage continuation exceeds the request".to_string(),
                                )
                            })?;
                            origin = B256::ZERO;
                        }
                        None => pending = &[],
                    }
                    if pending.len() == previous_len && origin <= previous_origin {
                        return Err(SnapSyncError::InvalidRequest(
                            "storage range continuation does not advance".to_string(),
                        ))
                    }
                    excluded.clear();
                }
            }
        }
        Ok(Some(storages))
    }

    // Preserves missing hashes across truncated responses until the batch is complete.
    async fn download_bytecodes(
        &mut self,
        accounts: &[(B256, TrieAccount)],
    ) -> Result<Option<Vec<(B256, Bytes)>>, SnapSyncError>
    where
        C: SnapClient,
    {
        let mut seen = B256Set::default();
        let hashes = accounts
            .iter()
            .map(|(_, account)| account.code_hash)
            .filter(|hash| *hash != KECCAK256_EMPTY && seen.insert(*hash))
            .collect::<Vec<_>>();
        let mut bytecodes = Vec::with_capacity(hashes.len());

        for chunk in hashes.chunks(BYTECODE_HASHES_PER_REQUEST) {
            let mut pending = chunk.to_vec();
            let mut excluded = Vec::new();
            while !pending.is_empty() {
                let request = GetByteCodesMessage {
                    request_id: self.next_request_id()?,
                    hashes: pending,
                    response_bytes: STATE_RESPONSE_BYTES,
                };
                let downloader = BytecodeDownloader::new_with_options(
                    self.client,
                    request,
                    self.runtime.clone(),
                    request_options(&excluded),
                )
                .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
                match downloader.await {
                    Ok(BytecodeOutcome::Unavailable { peer_id }) => {
                        pending = chunk
                            .iter()
                            .copied()
                            .filter(|hash| !bytecodes.iter().any(|(stored, _)| stored == hash))
                            .collect();
                        push_peer(&mut excluded, peer_id);
                    }
                    Err(RequestError::UnsupportedCapability) => return Ok(None),
                    Err(error) => return Err(error.into()),
                    Ok(BytecodeOutcome::Verified(verified)) => {
                        pending = verified
                            .codes
                            .into_iter()
                            .filter_map(|(hash, code)| match code {
                                Some(code) => {
                                    bytecodes.push((hash, code));
                                    None
                                }
                                None => Some(hash),
                            })
                            .collect();
                        excluded.clear();
                    }
                }
            }
        }
        Ok(Some(bytecodes))
    }

    // Failing on wrap prevents a stale response from matching a new logical request.
    fn next_request_id(&mut self) -> Result<u64, SnapSyncError> {
        self.request_id = self.request_id.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidRequest("snap request id space exhausted".to_string())
        })?;
        Ok(self.request_id)
    }
}

/// Terminal result of one state-download attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateDownloadOutcome {
    /// Every account dependency was committed and the generation entered BAL catch-up.
    Complete {
        /// Updated durable generation.
        generation: SnapGeneration,
    },
    /// No eligible peer currently serves the first uncommitted range.
    Unavailable {
        /// Last fully committed generation position.
        generation: SnapGeneration,
    },
}

/// Prioritizes follow-ups while preserving the unavailable-peer set.
fn request_options(excluded: &[PeerId]) -> SnapRequestOptions {
    let priority = if excluded.is_empty() { Priority::Normal } else { Priority::High };
    SnapRequestOptions::new(priority).with_excluded_peers(excluded.to_vec())
}

/// Keeps exclusions stable and duplicate-free across retries.
fn push_peer(peers: &mut Vec<PeerId>, peer_id: PeerId) {
    if !peers.contains(&peer_id) {
        peers.push(peer_id);
    }
}

/// Returns the next trie key, or `None` when the inclusive keyspace is exhausted.
fn next_hash(hash: B256) -> Option<B256> {
    let mut bytes = [0u8; B256::len_bytes()];
    bytes.copy_from_slice(hash.as_slice());
    for byte in bytes.iter_mut().rev() {
        let (next, overflow) = byte.overflowing_add(1);
        *byte = next;
        if !overflow {
            return Some(B256::new(bytes))
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, U256};
    use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
    use reth_downloaders::snap::test_utils::TestSnapClient;
    use reth_eth_wire_types::snap::{
        AccountData, AccountRangeMessage, ByteCodesMessage, StorageData, StorageRangesMessage,
    };
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::WithPeerId;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_storage_api::StorageSettings;
    use reth_trie_common::{HashBuilder, Nibbles};

    fn response(peer_id: PeerId, response: SnapResponse) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(peer_id, response))
    }

    fn trie_root(entries: impl IntoIterator<Item = (B256, Vec<u8>)>) -> B256 {
        let mut builder = HashBuilder::default();
        for (hash, value) in entries {
            builder.add_leaf(Nibbles::unpack(hash), &value);
        }
        builder.root()
    }

    #[tokio::test]
    async fn empty_root_completes_state_download() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let generation = SnapGeneration::new(10, B256::repeat_byte(1), EMPTY_ROOT_HASH);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let client = TestSnapClient::new([response(
            PeerId::random(),
            SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: Vec::new(),
                proof: Vec::new(),
            }),
        )]);

        let outcome =
            StateDownloader::new(&client, &factory, Runtime::test()).run(generation).await.unwrap();

        let StateDownloadOutcome::Complete { generation } = outcome else {
            panic!("completed state download")
        };
        assert_eq!(generation.phase, SnapPhase::BlockAccessLists);
    }

    #[tokio::test]
    async fn unavailable_range_excludes_each_peer() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let generation = SnapGeneration::new(10, B256::repeat_byte(1), B256::repeat_byte(2));
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let first = PeerId::random();
        let second = PeerId::random();
        let client = TestSnapClient::new([
            response(
                first,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: Vec::new(),
                    proof: Vec::new(),
                }),
            ),
            response(
                second,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 2,
                    accounts: Vec::new(),
                    proof: Vec::new(),
                }),
            ),
        ]);

        let outcome =
            StateDownloader::new(&client, &factory, Runtime::test()).run(generation).await.unwrap();

        assert_eq!(outcome, StateDownloadOutcome::Unavailable { generation });
        assert_eq!(*client.exclusions(), [vec![], vec![first], vec![first, second]]);
    }

    #[tokio::test]
    async fn commits_account_storage_and_bytecode_as_one_batch() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let account_hash = B256::repeat_byte(0x11);
        let slot_hash = B256::repeat_byte(0x22);
        let slot_value = U256::from(7);
        let storage_root = trie_root([(slot_hash, alloy_rlp::encode(slot_value))]);
        let code = Bytes::from_static(&[0x60, 0x00]);
        let code_hash = keccak256(&code);
        let account = TrieAccount { nonce: 3, balance: U256::from(4), storage_root, code_hash };
        let state_root = trie_root([(account_hash, alloy_rlp::encode(account))]);
        let generation = SnapGeneration::new(10, B256::repeat_byte(1), state_root);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let peer = PeerId::random();
        let client = TestSnapClient::new([
            response(
                peer,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: vec![AccountData::from_trie_account(account_hash, &account)],
                    proof: Vec::new(),
                }),
            ),
            response(
                peer,
                SnapResponse::StorageRanges(StorageRangesMessage {
                    request_id: 2,
                    slots: vec![vec![StorageData::from_value(slot_hash, slot_value)]],
                    proof: Vec::new(),
                }),
            ),
            response(
                peer,
                SnapResponse::ByteCodes(ByteCodesMessage {
                    request_id: 3,
                    codes: vec![code.clone()],
                }),
            ),
        ]);

        let outcome =
            StateDownloader::new(&client, &factory, Runtime::test()).run(generation).await.unwrap();

        assert!(matches!(outcome, StateDownloadOutcome::Complete { .. }));
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(account_hash).unwrap().unwrap().nonce,
            3
        );
        assert_eq!(
            provider
                .tx_ref()
                .get::<tables::Bytecodes>(code_hash)
                .unwrap()
                .unwrap()
                .original_bytes(),
            code
        );
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
        assert_eq!(
            cursor.seek_by_key_subkey(account_hash, slot_hash).unwrap().unwrap().value,
            slot_value
        );
    }

    #[test]
    fn next_hash_stops_at_keyspace_end() {
        assert_eq!(next_hash(B256::ZERO), Some(B256::with_last_byte(1)));
        assert_eq!(next_hash(MAX_HASH), None);
    }
}
