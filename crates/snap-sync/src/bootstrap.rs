//! Runs snap synchronization from pivot selection until its state is handed to the trie rebuild.
//!
//! Every step commits before the next one starts and the attempt record is re-read on each pass,
//! so a run stopped anywhere resumes from what the database holds.

use crate::{
    AccountRangeDownload, AccountRangeStep, BlockAccessListCatchUp, BytecodeDownload, BytecodeStep,
    CatchUpStep, SnapAccountStore, SnapAttemptStore, SnapCatchUpStore, SnapGeneration,
    SnapPivotPolicy, SnapStateVerifier, SnapSyncError, SnapSyncSession, SnapWrite,
    StorageRangeDownload, StorageRangeStep, VerifiedRange, DEFAULT_SCAN_CHUNK,
};
use alloy_eips::BlockNumHash;
use reth_db_api::transaction::DbTxMut;
use reth_network_p2p::snap::client::SnapClient;
use reth_storage_api::{
    BlockHashReader, DBProvider, DatabaseProviderFactory, HeaderProvider, MetadataProvider,
    MetadataWriter, StageCheckpointWriter, StateWriter,
};
use reth_storage_errors::provider::ProviderError;
use reth_tasks::Runtime;
use std::{fmt, future::Future};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Default number of account ranges committed between pivot checks.
///
/// Bounds how long a pivot can lag unnoticed while ranges download.
pub const DEFAULT_RANGES_PER_CHECK: usize = 64;

/// Drives snap synchronization until its downloaded state is ready for the trie rebuild.
///
/// Each pass moves a lagging pivot forward, applies the block access lists that carry the
/// downloaded state to it, fetches scheduled repairs again at it, then downloads account ranges
/// with their storage and code. An attempt
/// whose pivot leaves the canonical chain or outlives the served lists is abandoned and started
/// over.
pub struct SnapBootstrap<C, F, X> {
    factory: F,
    // Blocking database work runs off the async worker.
    runtime: Runtime,
    // Head, finality and peer progress come from the node running the sync.
    context: X,
    policy: SnapPivotPolicy,
    // Target of the attempt being driven, rebuilt from the attempt record on every pass.
    session: SnapSyncSession,
    ranges_per_check: usize,
    // Stops the run at the next step boundary, leaving committed progress in place.
    cancel: CancellationToken,
    accounts: AccountRangeDownload<C, F>,
    storage: StorageRangeDownload<C, F>,
    bytecode: BytecodeDownload<C, F>,
    catch_up: BlockAccessListCatchUp<C, F>,
}

impl<C: Clone, F: Clone, X> SnapBootstrap<C, F, X> {
    /// Creates a run that has not touched the network or the database yet.
    pub fn new(client: C, factory: F, runtime: Runtime, context: X) -> Self {
        let policy = SnapPivotPolicy::default();
        Self {
            accounts: AccountRangeDownload::new(client.clone(), factory.clone(), runtime.clone()),
            storage: StorageRangeDownload::new(client.clone(), factory.clone(), runtime.clone()),
            bytecode: BytecodeDownload::new(client.clone(), factory.clone(), runtime.clone()),
            catch_up: BlockAccessListCatchUp::new(client, factory.clone(), runtime.clone()),
            factory,
            runtime,
            context,
            policy,
            session: SnapSyncSession::new(policy),
            ranges_per_check: DEFAULT_RANGES_PER_CHECK,
            cancel: CancellationToken::new(),
        }
    }
}

impl<C, F, X> SnapBootstrap<C, F, X> {
    /// Returns this run anchoring and re-anchoring pivots with `policy`.
    pub fn with_policy(mut self, policy: SnapPivotPolicy) -> Self {
        self.policy = policy;
        self.session = SnapSyncSession::new(policy);
        self
    }

    /// Returns this run committing at most `ranges` account ranges between pivot checks, at
    /// least one.
    pub const fn with_ranges_per_check(mut self, ranges: usize) -> Self {
        self.ranges_per_check = if ranges == 0 { 1 } else { ranges };
        self
    }

    /// Returns this run stopping once `cancel` fires.
    pub fn with_cancellation(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }
}

impl<C, F, X> SnapBootstrap<C, F, X>
where
    C: SnapClient + Clone + Unpin,
    F: DatabaseProviderFactory + Clone + 'static,
    F::Provider: HeaderProvider + MetadataProvider + BlockHashReader,
    F::ProviderRW: HeaderProvider
        + BlockHashReader
        + MetadataProvider
        + MetadataWriter
        + StageCheckpointWriter
        + StateWriter
        + DBProvider<Tx: DbTxMut>,
    X: SnapSyncContext,
{
    /// Runs until the downloaded state is handed to the merkle stage, or the run stops.
    ///
    /// Peers that do not serve the state, or headers not downloaded yet, wait for the context to
    /// report progress instead of failing the run.
    pub async fn run(&mut self) -> Result<SnapBootstrapOutcome, SnapSyncError> {
        loop {
            if self.cancel.is_cancelled() {
                return Ok(SnapBootstrapOutcome::Stopped)
            }
            let head = self.context.head()?;
            let step = match self.resolve(head)? {
                Resolved::Verified(pivot) => return Ok(SnapBootstrapOutcome::Verified { pivot }),
                Resolved::Waiting => {
                    debug!(target: "sync::snap", head, "No eligible snap pivot");
                    Step::Wait
                }
                Resolved::Active(write) => match self.drive(write, head).await {
                    Ok(step) => step,
                    Err(error) if error.is_transient() => {
                        debug!(target: "sync::snap", %error, "Waiting for peers or headers");
                        Step::Wait
                    }
                    Err(error) if error.is_reorg() => {
                        info!(target: "sync::snap", %error, "Snap pivot was reorged, restarting");
                        Step::Restart
                    }
                    // Progress this build cannot read is unusable, so the attempt starts over.
                    Err(error @ SnapSyncError::UnsupportedRecord { .. }) => {
                        info!(target: "sync::snap", %error, "Snap progress is unreadable, restarting");
                        Step::Restart
                    }
                    Err(SnapSyncError::Cancelled) => Step::Stop,
                    Err(error) => return Err(error),
                },
            };
            match step {
                Step::HandedOff(write) => {
                    let pivot = self.pivot()?;
                    info!(target: "sync::snap", ?pivot, "Snap state handed to the trie rebuild");
                    return Ok(SnapBootstrapOutcome::TrieRebuild { write, pivot })
                }
                Step::Continue => {}
                Step::Wait => {
                    if !self.wait(head).await {
                        return Ok(SnapBootstrapOutcome::Stopped)
                    }
                }
                Step::Restart => {
                    let provider = self.factory.database_provider_rw()?;
                    provider.abandon_snap_attempt()?;
                    provider.commit()?;
                }
                Step::Stop => return Ok(SnapBootstrapOutcome::Stopped),
            }
        }
    }

    // Resumes the recorded attempt while its pivot is canonical, otherwise starts one at the
    // pivot under `head`. These are single-record writes, cheap enough for the async worker.
    fn resolve(&mut self, head: u64) -> Result<Resolved, SnapSyncError> {
        let provider = self.factory.database_provider_rw()?;
        let mut session = SnapSyncSession::new(self.policy);
        if let Some(attempt) = provider.snap_attempt()? {
            if attempt.is_verified() {
                return Ok(Resolved::Verified(attempt.pivot()))
            }
            if let Some(write) = provider.active_snap_write()? {
                let generation = SnapGeneration::new(attempt.pivot(), attempt.state_root());
                if generation.is_canonical(&provider)? {
                    session.resume(generation);
                    self.session = session;
                    return Ok(Resolved::Active(write))
                }
                info!(target: "sync::snap", pivot = ?attempt.pivot(), "Snap pivot was reorged, restarting");
            }
        }

        session.select(&provider, head, self.context.finalized())?;
        let Some((generation, _)) = session.start() else { return Ok(Resolved::Waiting) };
        let write = provider.start_snap_attempt(generation)?;
        provider.start_account_coverage(write)?;
        provider.commit()?;
        info!(
            target: "sync::snap",
            pivot = ?generation.target(),
            state_root = %generation.state_root(),
            "Started snap attempt"
        );
        self.session = session;
        Ok(Resolved::Active(write))
    }

    // One pass over the attempt: pivot, catch-up, then up to `ranges_per_check` account ranges.
    async fn drive(&mut self, write: SnapWrite, head: u64) -> Result<Step, SnapSyncError> {
        let (applied, covered) = {
            let provider = self.factory.database_provider_ro()?;
            if provider.is_trie_rebuild_started(write)? {
                return Ok(Step::HandedOff(write))
            }
            let applied = provider
                .catch_up_progress(write)?
                .ok_or(SnapSyncError::NoCatchUpProgress)?
                .applied();
            let covered = provider.account_coverage(write)?.is_some_and(|c| c.is_complete());
            (applied, covered)
        };
        // Complete state carried to its pivot needs no further lists, only its trie rebuilt, so
        // neither their retention nor a newer pivot applies to it.
        if covered && applied == self.pivot()? {
            if let Some(step) = self.repair().await? {
                return Ok(step)
            }
            return self.hand_off(write).await
        }
        // Catch-up continues from the last applied block, not the pivot, so once that block's
        // successor is no longer served the state cannot be carried to any newer pivot.
        if !self.policy.is_catchable_from(applied.number, head) {
            info!(target: "sync::snap", ?applied, head, "Snap catch-up outlived the served block access lists, restarting");
            return Ok(Step::Restart)
        }
        let write = self.advance_pivot(write, head)?;
        // Lists only update accounts already downloaded, so coverage grows once they reach the
        // pivot.
        if let Some(step) = self.catch_up(write).await? {
            return Ok(step)
        }
        // Repairs take the pivot's values, which the rest of the state holds once the lists reach
        // it.
        if let Some(step) = self.repair().await? {
            return Ok(step)
        }
        self.download_ranges(write).await
    }

    // Moves a lagging pivot forward, since a few lists cost less than downloading state peers no
    // longer serve. Returns the write the attempt accepts afterwards.
    fn advance_pivot(&mut self, write: SnapWrite, head: u64) -> Result<SnapWrite, SnapSyncError> {
        let provider = self.factory.database_provider_rw()?;
        let from = self.pivot()?;
        let Some(advanced) =
            self.session.advance(&provider, write, head, self.context.finalized())?
        else {
            return Ok(write)
        };
        // The database takes one writer at a time, so this commits before any download does.
        provider.commit()?;
        info!(target: "sync::snap", ?from, to = ?self.pivot()?, "Advanced snap pivot");
        Ok(advanced)
    }

    // Applies lists until the downloaded state reaches the pivot. `Some` ends the pass.
    async fn catch_up(&mut self, write: SnapWrite) -> Result<Option<Step>, SnapSyncError> {
        let pivot = self.pivot()?.number;
        loop {
            match self.catch_up.next(write, pivot).await? {
                CatchUpStep::Complete => return Ok(None),
                CatchUpStep::Applied { progress, .. } => {
                    debug!(target: "sync::snap", applied = ?progress.applied(), pivot, "Applied block access lists");
                }
                CatchUpStep::Unavailable { .. } => return Ok(Some(Step::Wait)),
            }
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
    }

    // Fetches every scheduled repair again at the pivot, with the slots and code it needs. `Some`
    // ends the pass.
    async fn repair(&mut self) -> Result<Option<Step>, SnapSyncError> {
        loop {
            let range = match self.accounts.next_repair().await? {
                None => return Ok(None),
                Some(AccountRangeStep::Verified(range)) => range,
                Some(AccountRangeStep::Unavailable { .. }) => return Ok(Some(Step::Wait)),
            };
            let Some(slots) = self.storage.repair_slots(&range).await? else {
                return Ok(Some(Step::Wait))
            };
            if let Some(step) = self.download_code(&range).await? {
                return Ok(Some(step))
            }
            let hashed_address = range.origin();
            let remaining = self.accounts.commit_repair(range, slots).await?;
            debug!(target: "sync::snap", %hashed_address, remaining, "Repaired snap account");
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
    }

    // Commits up to `ranges_per_check` account ranges, handing the state off once none remain.
    async fn download_ranges(&mut self, write: SnapWrite) -> Result<Step, SnapSyncError> {
        for _ in 0..self.ranges_per_check {
            if self.cancel.is_cancelled() {
                return Ok(Step::Stop)
            }
            let range = match self.accounts.next().await? {
                Some(AccountRangeStep::Verified(range)) => range,
                Some(AccountRangeStep::Unavailable { .. }) => return Ok(Step::Wait),
                None => return self.hand_off(write).await,
            };
            if let Some(step) = self.download_storage_and_code(&range).await? {
                return Ok(step)
            }
            self.accounts.commit(range, Default::default(), Vec::new()).await?;
        }
        Ok(Step::Continue)
    }

    // Persists the storage and code `range` needs. `Some` ends the pass. Both commit as they
    // arrive, so a range dropped here is fetched again without repeating them.
    async fn download_storage_and_code(
        &mut self,
        range: &VerifiedRange,
    ) -> Result<Option<Step>, SnapSyncError> {
        loop {
            match self.storage.next(range).await? {
                StorageRangeStep::Complete => break,
                StorageRangeStep::Committed(_) => {}
                StorageRangeStep::Unavailable { .. } => return Ok(Some(Step::Wait)),
            }
            // A large contract takes many responses, each committed, so any of them is a
            // resumable place to stop.
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
        self.download_code(range).await
    }

    // Persists the code `range` references. `Some` ends the pass.
    async fn download_code(
        &mut self,
        range: &VerifiedRange,
    ) -> Result<Option<Step>, SnapSyncError> {
        loop {
            match self.bytecode.next(range).await? {
                BytecodeStep::Complete => return Ok(None),
                BytecodeStep::Committed { .. } => {}
                BytecodeStep::Unavailable { .. } => return Ok(Some(Step::Wait)),
            }
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
    }

    // Checks the downloaded state is complete and hands it to the merkle stage. The scan reads
    // every account, so it runs on the blocking pool.
    async fn hand_off(&self, write: SnapWrite) -> Result<Step, SnapSyncError> {
        let factory = self.factory.clone();
        let cancel = self.cancel.clone();
        self.runtime
            .spawn_blocking(move || -> Result<(), SnapSyncError> {
                let provider = factory.database_provider_rw()?;
                provider.start_trie_rebuild(write, DEFAULT_SCAN_CHUNK, &cancel)?;
                provider.commit()?;
                Ok(())
            })
            .await
            .map_err(|error| SnapSyncError::Provider(ProviderError::other(error)))??;
        Ok(Step::HandedOff(write))
    }

    // Pivot of the attempt being driven.
    fn pivot(&self) -> Result<BlockNumHash, SnapSyncError> {
        self.session.target().map(SnapGeneration::target).ok_or(SnapSyncError::NoAttempt)
    }

    // Whether the context reported progress before the run was cancelled.
    async fn wait(&mut self, head: u64) -> bool {
        let Self { cancel, context, .. } = self;
        cancel.run_until_cancelled(context.wait_for_progress(head)).await.unwrap_or(false)
    }
}

impl<C, F, X> fmt::Debug for SnapBootstrap<C, F, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapBootstrap")
            .field("policy", &self.policy)
            .field("session", &self.session)
            .field("ranges_per_check", &self.ranges_per_check)
            .finish_non_exhaustive()
    }
}

/// How a [`SnapBootstrap`] run ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapBootstrapOutcome {
    /// The downloaded state is complete and handed to the merkle stage; once the stage reaches
    /// `pivot`, [`SnapStateVerifier::verify_state_root`] accepts it under `write`.
    TrieRebuild {
        /// Write the state was downloaded under.
        write: SnapWrite,
        /// Block the state is anchored to.
        pivot: BlockNumHash,
    },
    /// An earlier run's state was already verified, so there is nothing to download.
    Verified {
        /// Block the verified state is anchored to.
        pivot: BlockNumHash,
    },
    /// The run was cancelled or the context reported no further progress. Committed progress is
    /// kept for the next run.
    Stopped,
}

/// What a [`SnapBootstrap`] needs to know about the chain and peers it synchronizes from.
pub trait SnapSyncContext: Send {
    /// Highest block whose canonical header is downloaded.
    fn head(&self) -> Result<u64, SnapSyncError>;

    /// Latest finalized block, preferred as the pivot when recent enough.
    fn finalized(&self) -> Option<u64> {
        None
    }

    /// Waits until the head moves past `head` or new peers connect, returning `false` once no
    /// further progress will come.
    fn wait_for_progress(&mut self, head: u64) -> impl Future<Output = bool> + Send;
}

// Whether the attempt being driven can continue, and how.
enum Resolved {
    // An unfinished attempt, resumed or just started, accepting this write.
    Active(SnapWrite),
    // An earlier run's state was verified at this pivot, so nothing is left to download.
    Verified(BlockNumHash),
    // No block is eligible as a pivot yet.
    Waiting,
}

// What one pass over the attempt left to do.
enum Step {
    HandedOff(SnapWrite),
    // The range budget ran out; the pivot is checked again before more ranges.
    Continue,
    // Peers or headers are missing until the node progresses.
    Wait,
    // The attempt's state can no longer be carried forward.
    Restart,
    // The run was cancelled; committed progress stays for the next one.
    Stop,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, account_range, hashed_factory, header, key, policy, state_root,
            storage_ranges, storage_root_of, verified_range, ScriptedSnapClient,
        },
        StateRepairs,
    };
    use alloy_eip7928::{compute_block_access_list_hash, AccountChanges};
    use alloy_eips::eip7928::bal::Bal;
    use alloy_primitives::{Bytes, B256, U256};
    use reth_eth_wire_types::{snap::BlockAccessListsMessage, BlockAccessLists};
    use reth_network_p2p::{
        error::{PeerRequestResult, RequestError},
        snap::client::SnapResponse,
    };
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::SealedHeader;
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        ProviderFactory,
    };
    use reth_stages::stages::MerkleStage;
    use reth_stages_api::{ExecInput, Stage};
    use reth_stages_types::StageId;
    use reth_storage_api::{SnapAttemptId, StageCheckpointReader};
    use reth_trie_common::TrieAccount;
    use std::{cell::RefCell, collections::VecDeque, sync::Arc};

    type Factory = ProviderFactory<MockNodeTypesWithDB>;
    type Bootstrap = SnapBootstrap<Arc<ScriptedSnapClient>, Factory, TestContext>;

    const FAR: B256 = B256::repeat_byte(0xaa);

    fn accounts() -> Vec<(B256, TrieAccount)> {
        vec![(key(1), account(1)), (key(2), account(2)), (FAR, account(3))]
    }

    // Blocks `0..=tip`, each committing to an empty list and to `root`, so any of them anchors
    // the same state.
    fn insert_chain(factory: &Factory, tip: u64, root: B256) {
        let commitment = compute_block_access_list_hash(&Vec::<AccountChanges>::new());
        let mut parent = B256::ZERO;
        let headers: Vec<_> = (0..=tip)
            .map(|number| {
                let mut header = header(number, parent, Some(commitment));
                header.state_root = root;
                let sealed = SealedHeader::seal_slow(header);
                parent = sealed.hash();
                sealed
            })
            .collect();
        insert_headers(factory, &headers);
    }

    // A peer serving `blocks` empty lists.
    fn empty_lists(request_id: u64, blocks: usize) -> PeerRequestResult<SnapResponse> {
        let list: Bytes = alloy_rlp::encode(Bal::from(Vec::<AccountChanges>::new())).into();
        let message = BlockAccessListsMessage {
            request_id,
            block_access_lists: BlockAccessLists(vec![Some(list); blocks]),
        };
        Ok(WithPeerId::new(PeerId::random(), SnapResponse::BlockAccessLists(message)))
    }

    // A peer holding no list for the first block it is asked for.
    fn no_lists(request_id: u64) -> PeerRequestResult<SnapResponse> {
        let message = BlockAccessListsMessage {
            request_id,
            block_access_lists: BlockAccessLists(vec![None]),
        };
        Ok(WithPeerId::new(PeerId::random(), SnapResponse::BlockAccessLists(message)))
    }

    fn scripted(
        factory: &Factory,
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
        heads: impl IntoIterator<Item = u64>,
    ) -> (Arc<ScriptedSnapClient>, Bootstrap) {
        let client = Arc::new(ScriptedSnapClient::new(responses));
        let context = TestContext { heads: RefCell::new(heads.into_iter().collect()), waits: 0 };
        let bootstrap =
            SnapBootstrap::new(Arc::clone(&client), factory.clone(), Runtime::test(), context)
                .with_policy(policy());
        (client, bootstrap)
    }

    fn attempt_id(factory: &Factory) -> SnapAttemptId {
        factory.database_provider_ro().unwrap().snap_attempt().unwrap().unwrap().id()
    }

    #[tokio::test]
    async fn downloads_the_state_and_hands_it_to_the_trie_rebuild() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let (client, mut bootstrap) =
            scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [3]);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { write, pivot } = outcome else {
            panic!("the state is complete: {outcome:?}")
        };
        assert_eq!(pivot.number, 2);
        assert_eq!(*client.origins(), [B256::ZERO]);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.is_trie_rebuild_started(write).unwrap());
    }

    #[tokio::test]
    async fn scheduled_repairs_are_fetched_at_the_pivot_before_ranges() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(2).unwrap().unwrap().num_hash();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(pivot, state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let mut repairs = StateRepairs::default();
        repairs.insert_account(key(2));
        provider.schedule_snap_repairs(write, repairs).unwrap();
        provider.commit().unwrap();
        let responses =
            // One download fetches the repair and then the ranges, numbering both requests.
            [account_range(1, &accounts, 1..2, &[key(2)]), account_range(2, &accounts, 0..3, &[])];
        let (client, mut bootstrap) = scripted(&factory, responses, [3]);

        let outcome = bootstrap.run().await.unwrap();

        assert!(matches!(outcome, SnapBootstrapOutcome::TrieRebuild { .. }), "{outcome:?}");
        assert_eq!(*client.origins(), [key(2), B256::ZERO]);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.snap_repairs(write).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_lagging_pivot_advances_before_more_ranges_download() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        insert_chain(&factory, 8, root);
        let responses = [
            account_range(1, &accounts, 0..1, &[key(1)]),
            // Blocks 3 through 7 carry the first account from pivot 2 to pivot 7.
            empty_lists(1, 5),
            account_range(2, &accounts, 1..3, &[key(2), FAR]),
        ];
        // The head moves past the advance window after the first range.
        let (client, bootstrap) = scripted(&factory, responses, [3, 8]);
        let mut bootstrap = bootstrap.with_ranges_per_check(1);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { write, pivot } = outcome else {
            panic!("the state is complete: {outcome:?}")
        };
        assert_eq!(pivot.number, 7);
        assert_eq!(*client.origins(), [B256::ZERO, key(2)]);
        assert_eq!(client.block_requests().len(), 1);

        // The merkle stage rebuilds the trie, which the pivot's header then accepts.
        let provider = factory.database_provider_rw().unwrap();
        let mut stage = MerkleStage::default_execution();
        loop {
            let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap();
            let output =
                stage.execute(&provider, ExecInput { target: Some(7), checkpoint }).unwrap();
            provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint).unwrap();
            if output.done {
                break
            }
        }
        let verified = provider.verify_state_root(write).unwrap();
        assert_eq!((verified.target(), verified.state_root()), (pivot, root));
    }

    #[tokio::test]
    async fn peers_without_the_state_wait_and_the_next_run_resumes() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let responses = [
            account_range(1, &accounts, 0..1, &[key(1)]),
            Err(RequestError::UnsupportedCapability),
        ];
        let (_, mut bootstrap) = scripted(&factory, responses, [3]);

        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        assert_eq!(bootstrap.context.waits, 1);
        let attempt = attempt_id(&factory);

        let (client, mut resumed) =
            scripted(&factory, [account_range(1, &accounts, 1..3, &[key(2), FAR])], [3]);
        let outcome = resumed.run().await.unwrap();

        assert!(matches!(outcome, SnapBootstrapOutcome::TrieRebuild { .. }));
        assert_eq!(attempt_id(&factory), attempt);
        assert_eq!(*client.origins(), [key(2)]);
    }

    #[tokio::test]
    async fn no_eligible_pivot_waits_without_starting_an_attempt() {
        let factory = hashed_factory();
        insert_chain(&factory, 0, state_root(&accounts()));
        let (client, mut bootstrap) = scripted(&factory, [], [0]);

        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);

        assert_eq!(bootstrap.context.waits, 1);
        assert!(client.origins().is_empty());
        assert!(factory.database_provider_ro().unwrap().snap_attempt().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_reorged_pivot_restarts_the_attempt() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        // A previous run anchored to a block the canonical chain no longer holds.
        let provider = factory.database_provider_rw().unwrap();
        let orphaned = SnapGeneration::new(
            BlockNumHash::new(2, B256::repeat_byte(0xee)),
            state_root(&accounts),
        );
        provider.start_snap_attempt(orphaned).unwrap();
        provider.commit().unwrap();
        let orphaned = attempt_id(&factory);
        let (_, mut bootstrap) = scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [3]);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the state is complete: {outcome:?}")
        };
        assert_ne!(pivot.hash, B256::repeat_byte(0xee));
        assert_ne!(attempt_id(&factory), orphaned);
    }

    #[tokio::test]
    async fn a_handed_off_attempt_is_not_downloaded_again() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let (_, mut bootstrap) = scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [3]);
        let first = bootstrap.run().await.unwrap();

        let (client, mut resumed) = scripted(&factory, [], [3]);

        assert_eq!(resumed.run().await.unwrap(), first);
        assert!(client.origins().is_empty());
    }

    #[tokio::test]
    async fn a_cancelled_run_stops_before_any_work() {
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts()));
        let cancel = CancellationToken::new();
        let (client, bootstrap) = scripted(&factory, [], [3]);
        let mut bootstrap = bootstrap.with_cancellation(cancel.clone());
        cancel.cancel();

        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        assert!(client.origins().is_empty());
    }

    #[tokio::test]
    async fn a_catch_up_stalled_past_the_served_lists_restarts() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 11, state_root(&accounts));
        // The pivot moves from 2 to 7, but no peer serves block 3's list.
        let responses = [account_range(1, &accounts, 0..1, &[key(1)]), no_lists(1)];
        let (_, stalled) = scripted(&factory, responses, [3, 8]);
        let mut stalled = stalled.with_ranges_per_check(1);
        assert_eq!(stalled.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        let attempt = attempt_id(&factory);

        // Pivot 7 is still recent under head 11, but block 3's list is past the served history.
        let (client, mut restarted) =
            scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [11]);
        let outcome = restarted.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the state is complete: {outcome:?}")
        };
        assert_eq!(pivot.number, 10);
        assert_ne!(attempt_id(&factory), attempt);
        assert!(client.block_requests().is_empty());
    }

    #[tokio::test]
    async fn complete_state_is_handed_off_past_the_served_lists() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 11, state_root(&accounts));
        // A run committed every range at pivot 2, then stopped before the hand-off.
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(2).unwrap().unwrap();
        let generation = SnapGeneration::new(pivot.num_hash(), pivot.state_root);
        let downloaded = provider.start_snap_attempt(generation).unwrap();
        provider.start_account_coverage(downloaded).unwrap();
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        provider.commit_account_range(downloaded, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();

        // Block 3's list is past the served history under head 11, but nothing needs it.
        let (client, mut resumed) = scripted(&factory, [], [11]);
        let outcome = resumed.run().await.unwrap();

        assert_eq!(
            outcome,
            SnapBootstrapOutcome::TrieRebuild { write: downloaded, pivot: pivot.num_hash() }
        );
        assert!(client.origins().is_empty());
        assert!(client.block_requests().is_empty());
    }

    #[tokio::test]
    async fn unreadable_progress_restarts_the_attempt() {
        let accounts = accounts();
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let (_, mut stopped) = scripted(&factory, [Err(RequestError::UnsupportedCapability)], [3]);
        assert_eq!(stopped.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        // Another build rewrote the coverage the attempt resumes from.
        let provider = factory.database_provider_rw().unwrap();
        let stale = provider.active_snap_write().unwrap().unwrap();
        provider.write_metadata("snap_account_coverage", br#"{"version":999}"#.to_vec()).unwrap();
        provider.commit().unwrap();

        let (_, mut restarted) = scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [3]);
        let outcome = restarted.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { write, .. } = outcome else {
            panic!("the state is complete: {outcome:?}")
        };
        assert_ne!(write.attempt(), stale.attempt());
        let provider = factory.database_provider_ro().unwrap();
        assert!(matches!(
            provider.authorize_snap_write(stale),
            Err(SnapSyncError::StaleWrite { .. })
        ));
    }

    #[tokio::test]
    async fn cancellation_between_storage_responses_stops_the_run() {
        let slots = vec![(key(1), U256::from(11)), (key(2), U256::from(12))];
        let mut contract = account(1);
        contract.storage_root = storage_root_of(&slots);
        let accounts = vec![(key(1), contract)];
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let cancel = CancellationToken::new();
        let on_request = cancel.clone();
        // The contract's storage takes two responses, and shutdown fires during the first.
        let client = Arc::new(
            ScriptedSnapClient::new([
                account_range(1, &accounts, 0..1, &[]),
                storage_ranges(1, &[&slots[..1]], &slots, &[B256::ZERO, key(1)]),
            ])
            .on_storage_request(move || on_request.cancel()),
        );
        let context = TestContext { heads: RefCell::new(VecDeque::from([3])), waits: 0 };
        let mut bootstrap =
            SnapBootstrap::new(Arc::clone(&client), factory.clone(), Runtime::test(), context)
                .with_policy(policy())
                .with_cancellation(cancel);

        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        assert_eq!(client.storage_requests().len(), 1);
    }

    // Serves heads in order, repeating the last, and ends the run at the first wait.
    struct TestContext {
        heads: RefCell<VecDeque<u64>>,
        waits: usize,
    }

    impl SnapSyncContext for TestContext {
        fn head(&self) -> Result<u64, SnapSyncError> {
            let mut heads = self.heads.borrow_mut();
            let head = if heads.len() > 1 { heads.pop_front() } else { heads.front().copied() };
            Ok(head.expect("the context is given at least one head"))
        }

        fn wait_for_progress(&mut self, _head: u64) -> impl Future<Output = bool> + Send {
            self.waits += 1;
            std::future::ready(false)
        }
    }
}
