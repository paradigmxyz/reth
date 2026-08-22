//! Sequences one Snap bootstrap from pivot selection through validated trie generation.
//!
//! The session owns every durable transition: it picks or resumes a generation, rolls the pivot
//! forward while ranges are still downloading, applies the final block access lists, and rebuilds
//! the trie. Phases that run out of peers wait for the node to report progress instead of spinning.

use crate::{
    error::db_error, BlockAccessListCatchUp, BlockAccessListCatchUpOutcome, RangeBudget,
    SnapGeneration, SnapPhase, SnapPivotPolicy, SnapStateStore, SnapSyncError,
    StateDownloadOutcome, StateDownloader, TrieGenerator,
};
use core::future::Future;
use reth_db_api::transaction::DbTxMut;
use reth_network_p2p::snap::client::SnapClient;
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{
    AccountExtReader, ChangeSetReader, DBProvider, HeaderProvider, StageCheckpointReader,
    StageCheckpointWriter, StateWriter, StatsReader, StorageChangeSetReader, StorageSettingsCache,
    TrieWriter,
};
use reth_tasks::Runtime;
use tracing::{debug, info};

// Bounding a download attempt keeps pivot re-anchoring on a predictable cadence.
const DEFAULT_RANGE_BUDGET: RangeBudget = RangeBudget::new(64);

/// Drives a Snap state bootstrap to a validated state root.
#[derive(Debug)]
pub struct SnapSyncSession<'a, C, F, X> {
    // A reference avoids requiring network clients to implement Clone.
    client: &'a C,
    // Every phase observes the same provider factory.
    factory: &'a F,
    // Generation transitions stay durable through the store.
    store: SnapStateStore<'a, F>,
    // Head and peer progress come from the node that owns the session.
    context: X,
    // Proof verification and trie generation stay off the async worker.
    runtime: Runtime,
    // Decides where generations are anchored and when they are abandoned.
    policy: SnapPivotPolicy,
    // Account ranges committed between pivot re-anchoring checks.
    budget: RangeBudget,
}

impl<'a, C, F, X> SnapSyncSession<'a, C, F, X> {
    /// Creates a session without starting network or database work.
    pub const fn new(client: &'a C, factory: &'a F, context: X, runtime: Runtime) -> Self {
        Self {
            client,
            factory,
            store: SnapStateStore::new(factory),
            context,
            runtime,
            policy: SnapPivotPolicy::new(),
            budget: DEFAULT_RANGE_BUDGET,
        }
    }

    /// Sets the pivot distance and history bounds the session enforces.
    pub const fn with_policy(mut self, policy: SnapPivotPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets how many account ranges are committed between pivot re-anchoring checks.
    pub const fn with_range_budget(mut self, budget: RangeBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Runs until the state is validated or the context reports no further progress.
    ///
    /// A pivot that leaves the canonical chain, or falls behind the block access list history
    /// peers serve, is abandoned and replaced by a fresh generation rather than failing the
    /// session.
    pub async fn run(&mut self) -> Result<SnapSyncOutcome, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory<Provider: HeaderProvider + StageCheckpointReader>
            + Clone
            + Send
            + 'static,
        F::ProviderRW: SnapSyncProvider,
        X: SnapSyncContext,
    {
        loop {
            let head = self.context.canonical_head()?;
            let Some(generation) = self.resolve_generation(head)? else {
                debug!(target: "snap::session", head, "No eligible snap pivot");
                if !self.context.wait_for_progress(head).await {
                    return Ok(SnapSyncOutcome::Stalled { generation: None })
                }
                continue
            };

            match self.drive(generation, head).await {
                Ok(SessionStep::Complete(generation)) => {
                    return Ok(SnapSyncOutcome::Complete { generation })
                }
                Ok(SessionStep::Stalled(generation)) => {
                    return Ok(SnapSyncOutcome::Stalled { generation: Some(generation) })
                }
                Ok(SessionStep::Restart) => {
                    info!(
                        target: "snap::session",
                        target_block = generation.target_block,
                        head,
                        "Snap pivot outlived the served BAL history, restarting"
                    );
                }
                Err(error) if is_reorg(&error) => {
                    info!(target: "snap::session", %error, "Snap pivot left the canonical chain, restarting");
                    if !self.context.wait_for_progress(head).await {
                        return Ok(SnapSyncOutcome::Stalled { generation: Some(generation) })
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    // Resumes a usable generation, otherwise starts a clean one on the current canonical pivot.
    fn resolve_generation(&self, head: u64) -> Result<Option<SnapGeneration>, SnapSyncError>
    where
        F: DatabaseProviderFactory<Provider: HeaderProvider + StageCheckpointReader>,
        F::ProviderRW: SnapSyncProvider,
    {
        let interrupted = self.store.interrupted_generation()?;
        let pivot = {
            let provider = self.factory.database_provider_ro().map_err(db_error)?;
            if let Some(generation) = interrupted &&
                self.policy.is_resumable(&provider, generation, head)?
            {
                return Ok(Some(generation))
            }
            self.policy.select(&provider, head)?
        };
        let Some(pivot) = pivot else { return Ok(None) };
        info!(
            target: "snap::session",
            target_block = pivot.target_block,
            state_root = %pivot.state_root,
            "Starting snap state generation"
        );
        self.store.begin_generation(pivot)?;
        Ok(Some(pivot))
    }

    // Advances one generation through its remaining phases, waiting whenever peers run out.
    async fn drive(
        &mut self,
        mut generation: SnapGeneration,
        mut head: u64,
    ) -> Result<SessionStep, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory<Provider: HeaderProvider + StageCheckpointReader>
            + Clone
            + Send
            + 'static,
        F::ProviderRW: SnapSyncProvider,
        X: SnapSyncContext,
    {
        // Both helpers borrow the session's inputs, not the session, so waits stay possible.
        let mut downloader = StateDownloader::new(self.client, self.factory, self.runtime.clone());
        let mut catch_up =
            BlockAccessListCatchUp::new(self.client, self.factory, self.runtime.clone());

        loop {
            match generation.phase {
                SnapPhase::Accounts => {
                    if !self.policy.is_catchable(generation.target_block, head) {
                        return Ok(SessionStep::Restart)
                    }
                    if let Some(target) = self.pending_advance(generation, head) {
                        match catch_up.advance_pivot(generation, target).await? {
                            BlockAccessListCatchUpOutcome::Complete { generation: next } => {
                                generation = next;
                            }
                            BlockAccessListCatchUpOutcome::Unavailable { generation: next } => {
                                generation = next;
                                let Some(next_head) = self.wait(head).await? else {
                                    return Ok(SessionStep::Stalled(generation))
                                };
                                head = next_head;
                                continue
                            }
                        }
                    }

                    match downloader.run(generation, self.budget).await? {
                        StateDownloadOutcome::Complete { generation: next } => generation = next,
                        StateDownloadOutcome::Paused { generation: next } => {
                            generation = next;
                            head = self.context.canonical_head()?;
                        }
                        StateDownloadOutcome::Unavailable { generation: next } => {
                            generation = next;
                            let Some(next_head) = self.wait(head).await? else {
                                return Ok(SessionStep::Stalled(generation))
                            };
                            head = next_head;
                        }
                    }
                }
                SnapPhase::BlockAccessLists => {
                    if !self.policy.is_catchable(generation.target_block, head) {
                        return Ok(SessionStep::Restart)
                    }
                    let target =
                        self.advance_target(generation, head).unwrap_or(generation.target_block);
                    match catch_up.run(generation, target).await? {
                        BlockAccessListCatchUpOutcome::Complete { generation: next } => {
                            generation = next;
                        }
                        BlockAccessListCatchUpOutcome::Unavailable { generation: next } => {
                            generation = next;
                            let Some(next_head) = self.wait(head).await? else {
                                return Ok(SessionStep::Stalled(generation))
                            };
                            head = next_head;
                        }
                    }
                }
                SnapPhase::Trie => {
                    info!(
                        target: "snap::session",
                        target_block = generation.target_block,
                        "Rebuilding snap state trie"
                    );
                    self.rebuild_trie(generation).await?;
                    return Ok(SessionStep::Complete(generation))
                }
            }
        }
    }

    // Trie generation is CPU-bound and must not occupy the session's async worker.
    async fn rebuild_trie(&self, generation: SnapGeneration) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory + Clone + Send + 'static,
        F::ProviderRW: SnapSyncProvider,
    {
        let factory = self.factory.clone();
        self.runtime
            .spawn_blocking(move || TrieGenerator::new(&factory).run(generation))
            .await
            .map_err(|error| SnapSyncError::Trie(error.to_string()))?
    }

    // Only a pivot that moves forward is worth the block access lists it costs to apply.
    fn advance_target(&self, generation: SnapGeneration, head: u64) -> Option<u64> {
        self.policy.pivot_block(head).filter(|target| *target > generation.target_block)
    }

    // Downloading ranges are re-anchored only once the pivot has fallen far enough behind.
    fn pending_advance(&self, generation: SnapGeneration, head: u64) -> Option<u64> {
        self.policy
            .needs_advance(generation.target_block, head)
            .then(|| self.advance_target(generation, head))
            .flatten()
    }

    // Returns the refreshed head, or `None` once the context reports no further progress.
    async fn wait(&mut self, head: u64) -> Result<Option<u64>, SnapSyncError>
    where
        X: SnapSyncContext,
    {
        if !self.context.wait_for_progress(head).await {
            return Ok(None)
        }
        self.context.canonical_head().map(Some)
    }
}

/// Terminal result of one Snap session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapSyncOutcome {
    /// State was assembled and its trie validated at this generation.
    Complete {
        /// Generation whose state root matches its canonical header.
        generation: SnapGeneration,
    },
    /// The context reported that no further progress is possible.
    Stalled {
        /// Durable generation left in place for the next session, if one was started.
        generation: Option<SnapGeneration>,
    },
}

/// Canonical head and peer progress observed by a Snap session.
///
/// The node owning the session implements this so that stalled phases wait on real network or
/// chain events rather than polling.
pub trait SnapSyncContext {
    /// Returns the highest canonical block whose header is available locally.
    fn canonical_head(&self) -> Result<u64, SnapSyncError>;

    /// Resolves when the head may have advanced past `head`, or new peers became available.
    ///
    /// Returning `false` ends the session, leaving any durable generation resumable.
    fn wait_for_progress(&mut self, head: u64) -> impl Future<Output = bool> + Send;
}

/// Provider capabilities a Snap session needs to assemble and validate state.
pub trait SnapSyncProvider:
    DBProvider<Tx: DbTxMut>
    + AccountExtReader
    + ChangeSetReader
    + HeaderProvider
    + StageCheckpointReader
    + StageCheckpointWriter
    + StateWriter
    + StatsReader
    + StorageChangeSetReader
    + StorageSettingsCache
    + TrieWriter
{
}

impl<T> SnapSyncProvider for T where
    T: DBProvider<Tx: DbTxMut>
        + AccountExtReader
        + ChangeSetReader
        + HeaderProvider
        + StageCheckpointReader
        + StageCheckpointWriter
        + StateWriter
        + StatsReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + TrieWriter
{
}

// Keeps phase sequencing independent of the session's public outcome type.
enum SessionStep {
    // The generation's trie was rebuilt and accepted.
    Complete(SnapGeneration),
    // The generation can no longer be finished and must be replaced.
    Restart,
    // No further progress is possible.
    Stalled(SnapGeneration),
}

// A reorged or missing anchor invalidates the generation, not the session.
const fn is_reorg(error: &SnapSyncError) -> bool {
    matches!(
        error,
        SnapSyncError::CanonicalHeaderMismatch { .. } |
            SnapSyncError::CanonicalStateRootMismatch { .. } |
            SnapSyncError::MissingHeader(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AccountRangeProgress;
    use alloy_consensus::Header;
    use alloy_primitives::{B256, KECCAK256_EMPTY, U256};
    use reth_db_api::{tables, transaction::DbTx};
    use reth_downloaders::snap::test_utils::TestSnapClient;
    use reth_eth_wire_types::snap::{AccountData, AccountRangeMessage};
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        ProviderFactory, StaticFileProviderFactory, StaticFileWriter,
    };
    use reth_stages_types::StageId;
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::StorageSettings;
    use reth_trie_common::{HashBuilder, HashedPostState, Nibbles, TrieAccount, EMPTY_ROOT_HASH};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    // Short distances keep header fixtures small without changing the session's decisions.
    fn policy() -> SnapPivotPolicy {
        SnapPivotPolicy { head_distance: 1, advance_after: 4, history: 8 }
    }

    fn account() -> TrieAccount {
        TrieAccount {
            nonce: 3,
            balance: U256::from(4),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        }
    }

    fn state_root(hash: B256, account: &TrieAccount) -> B256 {
        let mut builder = HashBuilder::default();
        builder.add_leaf(Nibbles::unpack(hash), &alloy_rlp::encode(account));
        builder.root()
    }

    // Every header carries a BAL commitment unless the test needs a pre-EIP-7928 chain.
    fn chain(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
        roots: impl IntoIterator<Item = B256>,
        block_access_lists: bool,
    ) -> Vec<B256> {
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut hashes = Vec::new();
        let mut parent = B256::ZERO;
        for (number, state_root) in roots.into_iter().enumerate() {
            let header = Header {
                number: number as u64,
                parent_hash: parent,
                state_root,
                block_access_list_hash: block_access_lists
                    .then(|| B256::with_last_byte(number as u8)),
                ..Default::default()
            };
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
            parent = hash;
            hashes.push(hash);
        }
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);
        hashes
    }

    fn account_range(accounts: Vec<(B256, TrieAccount)>) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            PeerId::random(),
            SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: accounts
                    .iter()
                    .map(|(hash, account)| AccountData::from_trie_account(*hash, account))
                    .collect(),
                proof: Vec::new(),
            }),
        ))
    }

    #[tokio::test]
    async fn assembles_downloads_catch_up_and_trie_in_one_session() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let account_hash = B256::repeat_byte(0x11);
        let account = account();
        let root = state_root(account_hash, &account);
        let hashes = chain(&factory, [B256::ZERO, root], true);
        let client = TestSnapClient::new([account_range(vec![(account_hash, account)])]);
        let context = TestContext::new(2, []);

        let outcome = SnapSyncSession::new(&client, &factory, context.clone(), Runtime::test())
            .with_policy(policy())
            .run()
            .await
            .unwrap();

        let SnapSyncOutcome::Complete { generation } = outcome else { panic!("completed session") };
        assert_eq!(generation.target_block, 1);
        assert_eq!(generation.target_hash, hashes[1]);
        assert_eq!(generation.state_root, root);
        assert!(context.waits().is_empty());
        let store = SnapStateStore::new(&factory);
        assert_eq!(store.interrupted_generation().unwrap(), None);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(account_hash).unwrap(),
            Some(Account::from(account))
        );
        assert_eq!(
            provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap().unwrap().block_number,
            1
        );
    }

    #[tokio::test]
    async fn generation_outside_the_bal_window_restarts_on_a_fresh_pivot() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let hashes = chain(&factory, [B256::ZERO; 4], true);
        let store = SnapStateStore::new(&factory);
        let stale = SnapGeneration::new(0, hashes[0], B256::ZERO);
        store.begin_generation(stale).unwrap();
        let account_hash = B256::repeat_byte(0x11);
        store
            .commit_account_range(
                stale,
                HashedPostState::default()
                    .with_accounts([(account_hash, Some(Account::default()))]),
                Vec::new(),
                AccountRangeProgress::More { next_account: account_hash },
            )
            .unwrap();
        let client = TestSnapClient::new(std::iter::empty());
        let context = TestContext::new(3, []);

        let outcome = SnapSyncSession::new(&client, &factory, context.clone(), Runtime::test())
            .with_policy(SnapPivotPolicy { history: 1, ..policy() })
            .run()
            .await
            .unwrap();

        let SnapSyncOutcome::Stalled { generation } = outcome else { panic!("stalled session") };
        let generation = generation.expect("restarted generation");
        assert_eq!(generation.target_block, 2);
        assert_eq!(generation.target_hash, hashes[2]);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
        assert_eq!(context.waits(), [3]);
        // Restarting clears the abandoned generation's partial state.
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.tx_ref().get::<tables::HashedAccounts>(account_hash).unwrap(), None);
    }

    #[tokio::test]
    async fn resumable_generation_keeps_its_downloaded_state() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let hashes = chain(&factory, [B256::ZERO; 4], true);
        let store = SnapStateStore::new(&factory);
        let interrupted = SnapGeneration::new(2, hashes[2], B256::ZERO);
        store.begin_generation(interrupted).unwrap();
        let account_hash = B256::repeat_byte(0x11);
        let interrupted = store
            .commit_account_range(
                interrupted,
                HashedPostState::default()
                    .with_accounts([(account_hash, Some(Account::default()))]),
                Vec::new(),
                AccountRangeProgress::More { next_account: account_hash },
            )
            .unwrap();
        let client = TestSnapClient::new(std::iter::empty());
        let context = TestContext::new(3, []);

        let outcome = SnapSyncSession::new(&client, &factory, context.clone(), Runtime::test())
            .with_policy(policy())
            .run()
            .await
            .unwrap();

        assert_eq!(outcome, SnapSyncOutcome::Stalled { generation: Some(interrupted) });
        assert_eq!(store.interrupted_generation().unwrap(), Some(interrupted));
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.tx_ref().get::<tables::HashedAccounts>(account_hash).unwrap().is_some());
    }

    #[tokio::test]
    async fn chain_without_bal_commitments_waits_instead_of_polling() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        chain(&factory, [B256::ZERO; 4], false);
        let client = TestSnapClient::new(std::iter::empty());
        let context = TestContext::new(3, [3]);

        let outcome = SnapSyncSession::new(&client, &factory, context.clone(), Runtime::test())
            .with_policy(policy())
            .run()
            .await
            .unwrap();

        assert_eq!(outcome, SnapSyncOutcome::Stalled { generation: None });
        // One wait per unproductive head, and no request while no pivot is eligible.
        assert_eq!(context.waits(), [3, 3]);
        assert!(client.priorities().is_empty());
    }

    // Scripted head progression shared with the test that inspects it.
    #[derive(Clone, Debug)]
    struct TestContext(Arc<Mutex<TestProgress>>);

    #[derive(Debug)]
    struct TestProgress {
        head: u64,
        waits: Vec<u64>,
        heads: VecDeque<u64>,
    }

    impl TestContext {
        fn new(head: u64, heads: impl IntoIterator<Item = u64>) -> Self {
            Self(Arc::new(Mutex::new(TestProgress {
                head,
                waits: Vec::new(),
                heads: heads.into_iter().collect(),
            })))
        }

        fn waits(&self) -> Vec<u64> {
            self.0.lock().unwrap().waits.clone()
        }
    }

    impl SnapSyncContext for TestContext {
        fn canonical_head(&self) -> Result<u64, SnapSyncError> {
            Ok(self.0.lock().unwrap().head)
        }

        fn wait_for_progress(&mut self, head: u64) -> impl Future<Output = bool> + Send {
            let mut progress = self.0.lock().unwrap();
            progress.waits.push(head);
            let next = progress.heads.pop_front();
            if let Some(head) = next {
                progress.head = head;
            }
            core::future::ready(next.is_some())
        }
    }
}
