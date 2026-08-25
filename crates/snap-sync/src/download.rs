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

    /// Resumes account state download until `budget` is spent, the state completes, or every
    /// eligible peer is unavailable.
    pub async fn run(
        &mut self,
        mut generation: SnapGeneration,
        budget: RangeBudget,
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

        let mut remaining = budget.ranges();
        loop {
            let Some(next_remaining) = remaining.checked_sub(1) else {
                return Ok(StateDownloadOutcome::Paused { generation })
            };
            remaining = next_remaining;

            match self.download_range(generation).await? {
                RangeStep::Unavailable(generation) => {
                    return Ok(StateDownloadOutcome::Unavailable { generation })
                }
                RangeStep::Committed(committed) => {
                    generation = committed;
                    // Leaving the account phase means the trie was exhausted.
                    if generation.phase != SnapPhase::Accounts {
                        return Ok(StateDownloadOutcome::Complete { generation })
                    }
                }
            }
        }
    }

    /// Commits one authenticated account range, batch by batch.
    ///
    /// Each batch's storage and code are downloaded before the batch becomes durable, so an
    /// interruption never exposes accounts whose dependencies are still missing.
    async fn download_range(
        &mut self,
        mut generation: SnapGeneration,
    ) -> Result<RangeStep, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        let Some(range) = self
            .download_account_range(generation.state_root, generation.next_account, MAX_HASH)
            .await?
        else {
            return Ok(RangeStep::Unavailable(generation))
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
            return Ok(RangeStep::Committed(generation))
        }

        let total = range.accounts.len();
        for (index, accounts) in range.accounts.chunks(STATE_ACCOUNTS_PER_BATCH).enumerate() {
            let Some((state, bytecodes)) =
                self.download_batch(generation.state_root, accounts).await?
            else {
                return Ok(RangeStep::Unavailable(generation))
            };
            let committed = (index + 1) * STATE_ACCOUNTS_PER_BATCH;
            let progress = account_progress(accounts, committed >= total && !range.has_more)?;
            generation = self.store.commit_account_range(generation, state, bytecodes, progress)?;
        }
        Ok(RangeStep::Committed(generation))
    }

    // Storage and code complete before their accounts become durable, so `None` means the batch
    // must be retried rather than committed.
    async fn download_batch(
        &mut self,
        root: B256,
        accounts: &[AccountEntry],
    ) -> Result<Option<(HashedPostState, Vec<(B256, Bytes)>)>, SnapSyncError>
    where
        C: SnapClient,
    {
        let Some(storages) = self.download_storages(root, accounts).await? else { return Ok(None) };
        let Some(bytecodes) = self.download_bytecodes(accounts).await? else { return Ok(None) };
        let state = HashedPostState::default()
            .with_accounts(
                accounts.iter().map(|(hash, account)| (*hash, Some(Account::from(*account)))),
            )
            .with_storages(storages);
        Ok(Some((state, bytecodes)))
    }

    // Retries unavailable roots across distinct peers without penalizing them.
    async fn download_account_range(
        &mut self,
        root: B256,
        origin: B256,
        limit: B256,
    ) -> Result<Option<VerifiedAccountRange>, SnapSyncError>
    where
        C: SnapClient,
    {
        let mut excluded = Vec::new();
        loop {
            let request = GetAccountRangeMessage {
                request_id: self.next_request_id()?,
                root_hash: root,
                starting_hash: origin,
                limit_hash: limit,
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
        root: B256,
        accounts: &[AccountEntry],
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
                root_hash: root,
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
                    (pending, origin) =
                        storage_continuation(pending, origin, verified.continuation)?;
                    excluded.clear();
                }
            }
        }
        Ok(Some(storages))
    }

    // Preserves missing hashes across truncated responses until the batch is complete.
    async fn download_bytecodes(
        &mut self,
        accounts: &[AccountEntry],
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

/// A hashed account key with the trie value a range proof authenticated.
type AccountEntry = (B256, TrieAccount);

/// Where the next storage request resumes: the accounts still pending, and the slot to start at.
type StorageCursor<'a> = (&'a [AccountEntry], B256);

/// Resumes the next storage request, rejecting a continuation that would repeat this one.
fn storage_continuation(
    pending: &[AccountEntry],
    origin: B256,
    continuation: Option<StorageRangeContinuation>,
) -> Result<StorageCursor<'_>, SnapSyncError> {
    let (remaining, next_origin) = match continuation {
        Some(StorageRangeContinuation::Partial { account_index, starting_hash, .. }) => {
            (pending.get(account_index..), starting_hash)
        }
        Some(StorageRangeContinuation::NextAccount { account_index, .. }) => {
            (pending.get(account_index..), B256::ZERO)
        }
        // Every requested account was served in full.
        None => (Some(&[][..]), origin),
    };
    let remaining = remaining.ok_or_else(|| {
        SnapSyncError::InvalidRequest("storage continuation exceeds the request".to_string())
    })?;
    // Neither consuming an account nor moving the slot cursor would reissue the same request.
    if !remaining.is_empty() && remaining.len() == pending.len() && next_origin <= origin {
        return Err(SnapSyncError::InvalidRequest(
            "storage range continuation does not advance".to_string(),
        ))
    }
    Ok((remaining, next_origin))
}

/// Resumes at the hash after the last committed account, unless the range exhausted the trie.
fn account_progress(
    accounts: &[AccountEntry],
    complete: bool,
) -> Result<AccountRangeProgress, SnapSyncError> {
    if complete {
        return Ok(AccountRangeProgress::Complete)
    }
    let last = accounts.last().expect("account batch is non-empty").0;
    let next_account = next_hash(last).ok_or_else(|| {
        SnapSyncError::InvalidRequest("account range continues past the maximum hash".to_string())
    })?;
    Ok(AccountRangeProgress::More { next_account })
}

/// Whether one account range finished, or ran out of peers part way through its batches.
enum RangeStep {
    /// Every batch of the range is durable at this generation.
    Committed(SnapGeneration),
    /// No eligible peer served a batch; the generation is the last one committed.
    Unavailable(SnapGeneration),
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
    /// The range budget was spent while account ranges remained.
    Paused {
        /// Last fully committed generation position.
        generation: SnapGeneration,
    },
}

/// Number of account ranges a single download attempt commits before yielding.
///
/// A bounded budget lets a caller re-anchor the pivot between ranges instead of waiting for a
/// full-state download that can outlive the served BAL history.
///
/// # Examples
///
/// ```
/// use reth_snap_sync::RangeBudget;
///
/// assert_eq!(RangeBudget::new(4).ranges(), 4);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeBudget(usize);

impl RangeBudget {
    /// Downloads until the state completes or peers stop serving it.
    pub const UNBOUNDED: Self = Self(usize::MAX);

    /// Returns control after `ranges` committed account ranges.
    pub const fn new(ranges: usize) -> Self {
        Self(ranges)
    }

    /// Returns the number of account ranges left in the budget.
    pub const fn ranges(self) -> usize {
        self.0
    }
}

/// Prioritizes follow-ups while preserving the unavailable-peer set.
pub(crate) fn request_options(excluded: &[PeerId]) -> SnapRequestOptions {
    let priority = if excluded.is_empty() { Priority::Normal } else { Priority::High };
    SnapRequestOptions::new(priority).with_excluded_peers(excluded.iter().copied())
}

/// Keeps exclusions stable and duplicate-free across retries.
pub(crate) fn push_peer(peers: &mut Vec<PeerId>, peer_id: PeerId) {
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
    use reth_trie_common::{proof::ProofRetainer, HashBuilder, Nibbles};

    fn response(peer_id: PeerId, response: SnapResponse) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(peer_id, response))
    }

    fn empty_account(nonce: u64) -> TrieAccount {
        TrieAccount {
            nonce,
            balance: U256::from(1),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        }
    }

    fn root_and_proof(accounts: &[AccountEntry], targets: &[B256]) -> (B256, Vec<Bytes>) {
        let targets = targets.iter().copied().map(Nibbles::unpack).collect();
        let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
        for (hash, account) in accounts {
            builder.add_leaf(Nibbles::unpack(*hash), &alloy_rlp::encode(account));
        }
        let root = builder.root();
        let proof =
            builder.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, node)| node);
        (root, proof.collect())
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

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

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

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

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

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

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

    #[tokio::test]
    async fn spent_budget_pauses_before_the_next_range() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let first = (B256::repeat_byte(0x11), empty_account(1));
        let second = (B256::repeat_byte(0x22), empty_account(2));
        let (state_root, proof) = root_and_proof(&[first, second], &[first.0]);
        let generation = SnapGeneration::new(10, B256::repeat_byte(1), state_root);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let client = TestSnapClient::new([response(
            PeerId::random(),
            SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: vec![AccountData::from_trie_account(first.0, &first.1)],
                proof,
            }),
        )]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::new(1))
            .await
            .unwrap();

        let StateDownloadOutcome::Paused { generation } = outcome else {
            panic!("paused state download")
        };
        assert_eq!(generation.phase, SnapPhase::Accounts);
        assert_eq!(generation.next_account, next_hash(first.0).unwrap());
        // Only the first range was requested, so the peer never saw a continuation.
        assert_eq!(client.priorities().len(), 1);
    }

    #[test]
    fn next_hash_stops_at_keyspace_end() {
        assert_eq!(next_hash(B256::ZERO), Some(B256::with_last_byte(1)));
        assert_eq!(next_hash(MAX_HASH), None);
    }
}
