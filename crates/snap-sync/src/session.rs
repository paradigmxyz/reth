//! Drives one snap synchronization attempt: what it targets, and when it stops.

use crate::{
    account::account_progress, request::SnapRequests, AccountRangeProgress, SnapDownloadProgress,
    SnapGeneration, SnapPhase, SnapPivotPolicy, SnapStateStore, SnapSyncError,
    DEFAULT_RESPONSE_BYTES, MAX_HASH,
};
use alloy_primitives::{Bytes, B256};
use reth_downloaders::snap::VerifiedAccountBatch;
use reth_network_p2p::snap::client::SnapClient;
use reth_primitives_traits::Account;
use reth_provider::DatabaseProviderFactory;
use reth_tasks::Runtime;
use reth_trie_common::{HashedPostState, TrieAccount, EMPTY_ROOT_HASH};

use reth_storage_api::{
    DBProvider, HeaderProvider, StageCheckpointReader, StageCheckpointWriter, StateWriter,
    StorageSettingsCache,
};
use tokio_util::sync::CancellationToken;

// A 1 KiB estimate prevents small storage tries from overfilling a response request.
const STATE_ACCOUNTS_PER_BATCH: usize = DEFAULT_RESPONSE_BYTES as usize / 1024;

/// One snap attempt with a locally selected pivot and authenticated peer downloads.
#[derive(Debug)]
pub struct SnapSyncSession {
    // Decides which blocks are eligible targets.
    policy: SnapPivotPolicy,
    // How far the attempt has got.
    state: SnapSyncSessionState,
    // Cancelled once, watched by whatever took the target.
    cancellation: CancellationToken,
}

impl SnapSyncSession {
    /// Creates a session waiting for its first eligible target.
    pub fn new(policy: SnapPivotPolicy) -> Self {
        Self {
            policy,
            state: SnapSyncSessionState::Waiting,
            cancellation: CancellationToken::new(),
        }
    }

    /// What the session is doing.
    pub const fn state(&self) -> &SnapSyncSessionState {
        &self.state
    }

    /// Pivot this session is anchored to, if it has one.
    pub const fn target(&self) -> Option<&SnapGeneration> {
        self.state.target()
    }

    /// Returns whether this session has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// Selects or replaces a target until work starts; started work requires pivot advancement.
    pub fn select(
        &mut self,
        provider: &impl HeaderProvider,
        head: u64,
        finalized: Option<u64>,
    ) -> Result<&SnapSyncSessionState, SnapSyncError> {
        if matches!(self.state, SnapSyncSessionState::Waiting | SnapSyncSessionState::Selected(_)) {
            self.state = match self.policy.select(provider, head, finalized)? {
                Some(generation) => SnapSyncSessionState::Selected(generation),
                None => SnapSyncSessionState::Waiting,
            };
        }
        Ok(&self.state)
    }

    /// Hands the target and cancellation token to its downloader once; otherwise returns `None`.
    pub fn start(&mut self) -> Option<(SnapGeneration, CancellationToken)> {
        let SnapSyncSessionState::Selected(generation) = self.state else { return None };
        self.state = SnapSyncSessionState::Downloading(generation);
        Some((generation, self.cancellation.clone()))
    }

    /// Cancels outstanding work permanently; a later attempt needs a new session.
    pub fn cancel(&mut self) {
        self.cancellation.cancel();
        self.state = SnapSyncSessionState::Cancelled;
    }
}

/// What a [`SnapSyncSession`] is doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapSyncSessionState {
    /// No block is eligible yet, so the session holds no target.
    Waiting,
    /// A target is selected, but no work has taken it yet.
    Selected(SnapGeneration),
    /// Work is outstanding against the target.
    Downloading(SnapGeneration),
    /// The session was cancelled and its outstanding work signalled to stop.
    Cancelled,
}

impl SnapSyncSessionState {
    /// Target of this state, if it has one.
    pub const fn target(&self) -> Option<&SnapGeneration> {
        match self {
            Self::Selected(generation) | Self::Downloading(generation) => Some(generation),
            Self::Waiting | Self::Cancelled => None,
        }
    }
}

/// Downloads and durably assembles the flat state authenticated by one generation root.
#[derive(Debug)]
pub struct StateDownloader<'a, C, F> {
    requests: SnapRequests<'a, C>,
    // All state and cursor transitions share the same provider factory.
    store: SnapStateStore<'a, F>,
}

impl<'a, C, F> StateDownloader<'a, C, F> {
    /// Creates a state downloader without starting network or database work.
    pub const fn new(client: &'a C, factory: &'a F, runtime: Runtime) -> Self {
        Self { requests: SnapRequests::new(client, runtime), store: SnapStateStore::new(factory) }
    }

    /// Resumes account state download until `budget` is spent, the state completes, or every
    /// eligible peer is unavailable.
    pub async fn run(
        &mut self,
        mut generation: SnapDownloadProgress,
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

    // Commits batches only after their storage and bytecode are downloaded.
    async fn download_range(
        &mut self,
        mut generation: SnapDownloadProgress,
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
            .requests
            .download_account_range(generation.state_root, generation.next_account, MAX_HASH)
            .await?
        else {
            return Ok(RangeStep::Unavailable(generation))
        };

        if range.accounts().is_empty() {
            if range.has_more() {
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

        let total = range.accounts().len();
        let storage_accounts = range.storage_batch();
        let mut storage_start = 0;
        for start in (0..total).step_by(STATE_ACCOUNTS_PER_BATCH) {
            let end = (start + STATE_ACCOUNTS_PER_BATCH).min(total);
            let accounts = &range.accounts()[start..end];
            let storage_end = storage_start +
                accounts
                    .iter()
                    .filter(|(_, account)| account.storage_root != EMPTY_ROOT_HASH)
                    .count();
            let storage_batch = storage_accounts
                .range(storage_start..storage_end)
                .expect("storage batch covers the account chunk");
            let Some((state, bytecodes)) = self.download_batch(accounts, storage_batch).await?
            else {
                return Ok(RangeStep::Unavailable(generation))
            };
            storage_start = storage_end;
            let progress = account_progress(accounts, end >= total && !range.has_more())?;
            generation = self.store.commit_account_range(generation, state, bytecodes, progress)?;
        }
        Ok(RangeStep::Committed(generation))
    }

    // Storage and code complete before their accounts become durable, so `None` means the batch
    // must be retried rather than committed.
    async fn download_batch(
        &mut self,
        accounts: &[AccountEntry],
        storage_batch: VerifiedAccountBatch<'_>,
    ) -> Result<Option<(HashedPostState, Vec<(B256, Bytes)>)>, SnapSyncError>
    where
        C: SnapClient,
    {
        let Some(storages) = self.requests.download_storages(storage_batch).await? else {
            return Ok(None)
        };
        let Some(bytecodes) = self.requests.download_bytecodes(accounts).await? else {
            return Ok(None)
        };
        let state = HashedPostState::default()
            .with_accounts(
                accounts.iter().map(|(hash, account)| (*hash, Some(Account::from(*account)))),
            )
            .with_storages(storages);
        Ok(Some((state, bytecodes)))
    }
}

/// Terminal result of one state-download attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateDownloadOutcome {
    /// Every account dependency was committed and the generation entered BAL catch-up.
    Complete {
        /// Updated durable generation.
        generation: SnapDownloadProgress,
    },
    /// No eligible peer currently serves the first uncommitted range.
    Unavailable {
        /// Last fully committed generation position.
        generation: SnapDownloadProgress,
    },
    /// The range budget was spent while account ranges remained.
    Paused {
        /// Last fully committed generation position.
        generation: SnapDownloadProgress,
    },
}

/// Account ranges committed before yielding so callers can re-anchor within served BAL history.
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

/// A hashed account key with the trie value a range proof authenticated.
type AccountEntry = (B256, TrieAccount);

/// Whether one account range finished, or ran out of peers part way through its batches.
enum RangeStep {
    /// Every batch of the range is durable at this generation.
    Committed(SnapDownloadProgress),
    /// No eligible peer served a batch; the generation is the last one committed.
    Unavailable(SnapDownloadProgress),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{chain, policy, provider_with};

    fn session() -> SnapSyncSession {
        SnapSyncSession::new(policy())
    }

    #[test]
    fn waits_while_no_block_is_eligible() {
        // The only block access list commitment is at the head, past the head distance.
        let provider = provider_with(chain(Some(3)));
        let mut session = session();

        assert_eq!(session.select(&provider, 3, None).unwrap(), &SnapSyncSessionState::Waiting);
        assert_eq!(session.target(), None);
        assert!(session.start().is_none());
    }

    #[test]
    fn selects_an_eligible_target() {
        let headers = chain(Some(0));
        let expected = headers[2].clone();
        let provider = provider_with(headers);
        let mut session = session();

        session.select(&provider, 3, None).unwrap();

        let target = session.target().unwrap();
        assert_eq!(target.target().number, 2);
        assert_eq!(target.target().hash, expected.hash_slow());
    }

    #[test]
    fn a_target_no_work_took_is_replaced_as_the_head_advances() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();

        session.select(&provider, 2, None).unwrap();
        assert_eq!(session.target().unwrap().target().number, 1);

        session.select(&provider, 3, None).unwrap();
        assert_eq!(session.target().unwrap().target().number, 2);
    }

    #[test]
    fn a_target_no_work_took_is_dropped_once_it_is_no_longer_eligible() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 3, None).unwrap();

        // The head has run past the downloaded headers, so no candidate confirms the old target.
        session.select(&provider, 9, None).unwrap();

        assert_eq!(session.state(), &SnapSyncSessionState::Waiting);
        assert!(session.start().is_none());
    }

    #[test]
    fn a_target_work_took_is_kept() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 2, None).unwrap();
        let (started, _) = session.start().unwrap();

        session.select(&provider, 3, None).unwrap();

        assert_eq!(session.state(), &SnapSyncSessionState::Downloading(started));
    }

    #[test]
    fn only_one_worker_takes_a_target() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 3, None).unwrap();

        assert!(session.start().is_some());
        assert!(session.start().is_none());
    }

    #[test]
    fn cancellation_stops_outstanding_work() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 3, None).unwrap();
        let (_, outstanding) = session.start().unwrap();

        session.cancel();

        assert!(outstanding.is_cancelled());
        assert!(session.is_cancelled());
        assert_eq!(session.state(), &SnapSyncSessionState::Cancelled);
        assert_eq!(session.target(), None);
    }

    #[test]
    fn a_cancelled_session_selects_nothing() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.cancel();

        assert_eq!(session.select(&provider, 3, None).unwrap(), &SnapSyncSessionState::Cancelled);
        assert!(session.start().is_none());
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use alloy_primitives::{keccak256, KECCAK256_EMPTY, U256};
    use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
    use reth_downloaders::snap::test_utils::TestSnapClient;
    use reth_eth_wire_types::snap::{
        AccountData, AccountRangeMessage, ByteCodesMessage, StorageData, StorageRangesMessage,
    };
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::{PeerId, WithPeerId};
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
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), EMPTY_ROOT_HASH);
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
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), B256::repeat_byte(2));
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
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), state_root);
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
    async fn skips_empty_storage_accounts_in_a_mixed_range() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let empty_hash = B256::repeat_byte(0x11);
        let stored_hash = B256::repeat_byte(0x22);
        let slot_hash = B256::repeat_byte(0x33);
        let slot_value = U256::from(7);
        let storage_root = trie_root([(slot_hash, alloy_rlp::encode(slot_value))]);
        let empty = empty_account(1);
        let stored = TrieAccount {
            nonce: 2,
            balance: U256::from(3),
            storage_root,
            code_hash: KECCAK256_EMPTY,
        };
        let state_root = trie_root([
            (empty_hash, alloy_rlp::encode(empty)),
            (stored_hash, alloy_rlp::encode(stored)),
        ]);
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), state_root);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let peer = PeerId::random();
        let client = TestSnapClient::new([
            response(
                peer,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: vec![
                        AccountData::from_trie_account(empty_hash, &empty),
                        AccountData::from_trie_account(stored_hash, &stored),
                    ],
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
        ]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

        assert!(matches!(outcome, StateDownloadOutcome::Complete { .. }));
        let provider = factory.database_provider_ro().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
        assert_eq!(
            cursor.seek_by_key_subkey(stored_hash, slot_hash).unwrap().unwrap().value,
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
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), state_root);
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
}
