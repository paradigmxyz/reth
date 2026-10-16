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
use reth_network_p2p::{error::RequestError, snap::client::SnapClient};
use reth_primitives_traits::AlloyBlockHeader;
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
/// Each pass moves a lagging pivot forward, applies the lists carrying the state to it, refetches
/// scheduled repairs there, then downloads account ranges with storage and code. A reorg across the
/// pivot is repaired from the orphaned lists, while handed-off or expired state starts over.
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
    // Stops the all-or-nothing hand-off scan, which `cancel` otherwise stops.
    shutdown: Option<CancellationToken>,
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
            shutdown: None,
        }
    }
}

impl<C, F, X> SnapBootstrap<C, F, X> {
    /// Returns this run anchoring and re-anchoring pivots with `policy`.
    pub const fn with_policy(mut self, policy: SnapPivotPolicy) -> Self {
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

    /// Returns this run finishing the hand-off scan unless `shutdown` fires.
    ///
    /// The scan restarts from scratch once stopped, so [`Self::with_cancellation`] lets it finish.
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = Some(shutdown);
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
        // Waits recur every few seconds, so only the first for a reason logs at info until a pass
        // makes progress.
        let mut announced = None;
        loop {
            if self.cancel.is_cancelled() {
                return Ok(SnapBootstrapOutcome::Stopped)
            }
            let head = self.context.head()?;
            let step = match self.resolve(head)? {
                Resolved::Verified(pivot) => return Ok(SnapBootstrapOutcome::Verified { pivot }),
                Resolved::BeforeBlockAccessLists => {
                    return Ok(SnapBootstrapOutcome::BeforeBlockAccessLists)
                }
                Resolved::Waiting => {
                    if announced.replace(Wait::Pivot) == Some(Wait::Pivot) {
                        debug!(target: "sync::snap", head, "No eligible snap pivot yet, waiting for headers");
                    } else {
                        info!(target: "sync::snap", head, "No eligible snap pivot yet, waiting for headers");
                    }
                    Step::Wait
                }
                Resolved::Active(write) => match self.drive(write, head).await {
                    Ok(step) => step,
                    // The network fails snap requests at once while no connected peer serves them.
                    Err(SnapSyncError::Request(RequestError::UnsupportedCapability)) => {
                        if announced.replace(Wait::SnapPeer) == Some(Wait::SnapPeer) {
                            debug!(target: "sync::snap", "No connected peer serves snap/2, waiting for one");
                        } else {
                            info!(target: "sync::snap", "No connected peer serves snap/2, waiting for one");
                        }
                        Step::Wait
                    }
                    Err(error) if error.is_transient() => {
                        debug!(target: "sync::snap", %error, "Waiting for peers or headers");
                        Step::Wait
                    }
                    // The next pass finds the pivot orphaned and recovers or restarts it.
                    Err(error) if error.is_reorg() => {
                        debug!(target: "sync::snap", %error, "Snap pivot was reorged");
                        Step::Continue
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
                Step::Continue => announced = None,
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

    // Resumes the recorded attempt, even one whose pivot a reorg orphaned, otherwise starts one at
    // the pivot under `head`. Starting one reads the kept blocks and writes a few records, cheap
    // enough for the async worker.
    fn resolve(&mut self, head: u64) -> Result<Resolved, SnapSyncError> {
        let provider = self.factory.database_provider_rw()?;
        let mut session = SnapSyncSession::new(self.policy);
        if let Some(attempt) = provider.snap_attempt()? {
            if attempt.is_verified() {
                return Ok(Resolved::Verified(attempt.pivot()))
            }
            if let Some(write) = provider.active_snap_write()? {
                // An orphaned pivot is resumed too, so the pass can recover or restart it.
                session.resume(SnapGeneration::new(attempt.pivot(), attempt.state_root()));
                self.session = session;
                return Ok(Resolved::Active(write))
            }
        }

        session.select(&provider, head, self.context.finalized())?;
        let Some(generation) = session.start() else {
            // A head without a block access list predates them, so no block under it can anchor.
            // Once the staged pipeline executes past genesis, the node stays on it. The first
            // header pass may stop at the finalized block, so if block access lists
            // activated between it and the head, the node full syncs. That window is
            // narrow and accepted.
            return match provider.sealed_header(head)? {
                Some(header) if header.block_access_list_hash().is_none() => {
                    Ok(Resolved::BeforeBlockAccessLists)
                }
                _ => Ok(Resolved::Waiting),
            }
        };
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

    // One pass over the attempt: reorg recovery, pivot, catch-up, repairs, then up to
    // `ranges_per_check` account ranges.
    async fn drive(&mut self, write: SnapWrite, head: u64) -> Result<Step, SnapSyncError> {
        let (applied, complete) = {
            let provider = self.factory.database_provider_ro()?;
            let generation = self.session.target().copied().ok_or(SnapSyncError::NoAttempt)?;
            let handed_off = provider.is_trie_rebuild_started(write)?;
            if !generation.is_canonical(&provider)? {
                // Handed-off state may be partly rebuilt, so it cannot be repaired in place.
                if handed_off {
                    info!(target: "sync::snap", pivot = ?generation.target(), "Handed-off snap pivot was reorged, restarting");
                    return Ok(Step::Restart)
                }
                drop(provider);
                return self.recover(write, head).await
            }
            if handed_off {
                return Ok(Step::HandedOff(write))
            }
            let applied = provider
                .catch_up_progress(write)?
                .ok_or(SnapSyncError::NoCatchUpProgress)?
                .applied();
            // Repairs are fetched at the pivot, so pending ones still need it served.
            let complete = provider.account_coverage(write)?.is_some_and(|c| c.is_complete()) &&
                provider.snap_repairs(write)?.is_empty();
            (applied, complete)
        };
        // Complete state carried to its pivot needs no further lists, only its trie rebuilt, so
        // neither their retention nor a newer pivot applies to it.
        if complete && applied == self.pivot()? {
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

    // Repairs what a reorg across the pivot left in the downloaded state, as EIP-8189 describes:
    // the orphaned blocks' lists schedule what they changed for repair, and catch-up continues from
    // the last block both branches share. Unserved lists are waited for within the served state
    // window.
    async fn recover(&mut self, write: SnapWrite, head: u64) -> Result<Step, SnapSyncError> {
        let pivot = self.pivot()?;
        let provider = self.factory.database_provider_ro()?;
        let Some(reorg) = provider.snap_reorg(write)? else {
            info!(target: "sync::snap", ?pivot, "Snap pivot was reorged past its kept blocks, restarting");
            return Ok(Step::Restart)
        };
        let ancestor = reorg.ancestor();
        // Lists activate by timestamp, which grows along a chain, so an ancestor committing to one
        // means every block after it on either branch does too.
        let committed = provider
            .sealed_header(ancestor.number)?
            .is_some_and(|header| header.block_access_list_hash().is_some());
        if !committed {
            info!(target: "sync::snap", ?pivot, ?ancestor, "Snap pivot was reorged across block access list activation, restarting");
            return Ok(Step::Restart)
        }
        let resume = provider
            .catch_up_progress(write)?
            .ok_or(SnapSyncError::NoCatchUpProgress)?
            .resume_after(ancestor)
            .number;
        if !self.policy.is_catchable_from(resume, head) {
            info!(target: "sync::snap", ?pivot, ?ancestor, resume, head, "Orphaned block access lists expired, restarting");
            return Ok(Step::Restart)
        }
        let generation = self.policy.select(&provider, head, self.context.finalized())?;
        // The new pivot must descend from the ancestor, or the new branch is still too short.
        // Checked first, so waiting for it does not fetch the orphaned lists on every pass.
        let Some(generation) = generation.filter(|g| g.target().number >= ancestor.number) else {
            return Ok(Step::Wait)
        };
        drop(provider);
        // A peer lacking side-chain lists says nothing of the others, so another is asked later.
        let Some(lists) = self.catch_up.orphaned_lists(reorg.orphaned()).await? else {
            if !self.policy.awaits_orphaned_lists(ancestor.number, head) {
                info!(target: "sync::snap", ?pivot, ?ancestor, head, "Orphaned block access lists stayed unavailable, restarting");
                return Ok(Step::Restart)
            }
            debug!(target: "sync::snap", ?pivot, "Orphaned block access lists are unavailable");
            return Ok(Step::Wait)
        };

        // Scheduling reads every orphaned list, so it runs on the blocking pool.
        self.commit_blocking(move |provider| {
            provider.commit_reorg_recovery(write, ancestor, &lists, generation).map(|_| ())
        })
        .await?;
        info!(target: "sync::snap", ?pivot, ?ancestor, to = ?generation.target(), "Recovered snap state from a pivot reorg");
        Ok(Step::Continue)
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
                CatchUpStep::Unavailable { peer_id, .. } => {
                    debug!(target: "sync::snap", ?peer_id, pivot, "Peer does not serve the pivot's block access lists");
                    return Ok(Some(Step::Wait))
                }
            }
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
    }

    // Fetches scheduled repairs again at the pivot, with the slots and code they need, committing
    // each batch. `Some` ends the pass, at the latest after `ranges_per_check` commits so the pivot
    // is checked again.
    async fn repair(&mut self) -> Result<Option<Step>, SnapSyncError> {
        for _ in 0..self.ranges_per_check {
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
            debug!(target: "sync::snap", %hashed_address, remaining, "Committed snap repair batch");
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
        Ok(Some(Step::Continue))
    }

    // Commits up to `ranges_per_check` account ranges, handing the state off once none remain.
    async fn download_ranges(&mut self, write: SnapWrite) -> Result<Step, SnapSyncError> {
        for _ in 0..self.ranges_per_check {
            if self.cancel.is_cancelled() {
                return Ok(Step::Stop)
            }
            let range = match self.accounts.next().await? {
                Some(AccountRangeStep::Verified(range)) => range,
                Some(AccountRangeStep::Unavailable { origin, peer_id }) => {
                    debug!(target: "sync::snap", ?peer_id, %origin, "Peer does not serve the pivot state");
                    return Ok(Step::Wait)
                }
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
                StorageRangeStep::Unavailable { peer_id, .. } => {
                    debug!(target: "sync::snap", ?peer_id, "Peer does not serve the pivot's storage");
                    return Ok(Some(Step::Wait))
                }
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
                BytecodeStep::Unavailable { peer_id, .. } => {
                    debug!(target: "sync::snap", ?peer_id, "Peer does not serve the pivot's code");
                    return Ok(Some(Step::Wait))
                }
            }
            if self.cancel.is_cancelled() {
                return Ok(Some(Step::Stop))
            }
        }
    }

    // Checks the downloaded state is complete and hands it to the merkle stage. The scan reads
    // every account, so it runs on the blocking pool.
    async fn hand_off(&self, write: SnapWrite) -> Result<Step, SnapSyncError> {
        let cancel = self.shutdown.as_ref().unwrap_or(&self.cancel).clone();
        self.commit_blocking(move |provider| {
            provider.start_trie_rebuild(write, DEFAULT_SCAN_CHUNK, &cancel)
        })
        .await?;
        Ok(Step::HandedOff(write))
    }

    // Runs `apply` in one transaction on the blocking pool, committing only if it succeeds.
    async fn commit_blocking(
        &self,
        apply: impl FnOnce(&F::ProviderRW) -> Result<(), SnapSyncError> + Send + 'static,
    ) -> Result<(), SnapSyncError> {
        let factory = self.factory.clone();
        self.runtime
            .spawn_blocking(move || -> Result<(), SnapSyncError> {
                let provider = factory.database_provider_rw()?;
                apply(&provider)?;
                provider.commit()?;
                Ok(())
            })
            .await
            .map_err(|error| SnapSyncError::Provider(ProviderError::other(error)))?
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
    /// The head predates block access lists, so snap/2 can't sync this chain yet.
    BeforeBlockAccessLists,
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
    // The head predates block access lists, so no block can be a pivot.
    BeforeBlockAccessLists,
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

// Why a run waits, so a repeated wait logs at debug.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait {
    Pivot,
    SnapPeer,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, account_range, hashed_factory, header, key, policy, state_root,
            storage_ranges, storage_root_of, stored_slots, verified_range, ReorgFactoryExt,
            ScriptedSnapClient,
        },
        StateRepairs, VerifiedSnapState,
    };
    use alloy_eip7928::{
        compute_block_access_list_hash, AccountChanges, BalanceChange, BlockAccessIndex,
    };
    use alloy_primitives::{keccak256, Address, B256, U256};
    use reth_eth_wire_types::{
        snap::{AccountRangeMessage, BlockAccessListsMessage},
        BlockAccessLists,
    };
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::{Account, AlloyBlockHeader, SealedHeader};
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        ProviderFactory,
    };
    use reth_stages::stages::MerkleStage;
    use reth_stages_api::{ExecInput, Stage};
    use reth_stages_types::StageId;
    use reth_storage_api::{SnapAttemptId, StageCheckpointReader};
    use reth_trie_common::{HashedPostState, TrieAccount};
    use std::{cell::RefCell, collections::VecDeque, sync::Arc};

    type Factory = ProviderFactory<MockNodeTypesWithDB>;
    type Bootstrap = SnapBootstrap<Arc<ScriptedSnapClient>, Factory, TestContext>;

    const FAR: B256 = B256::repeat_byte(0xaa);
    // Changed on the orphaned branch, which credited it.
    const STALE: Address = Address::repeat_byte(0x51);
    // Untouched by the orphaned branch.
    const KEPT: Address = Address::repeat_byte(0x52);

    fn accounts() -> Vec<(B256, TrieAccount)> {
        vec![(key(1), account(1)), (key(2), account(2)), (FAR, account(3))]
    }

    // Blocks `0..=tip`, each committing to an empty list and to `root`, so any of them anchors
    // the same state.
    fn insert_chain(factory: &Factory, tip: u64, root: B256) {
        insert_headers(factory, &chain(tip, root));
    }

    // A peer serving `blocks` empty lists.
    fn empty_lists(request_id: u64, blocks: usize) -> PeerRequestResult<SnapResponse> {
        lists(request_id, &vec![Vec::new(); blocks], true)
    }

    // A peer holding no list for the first block it is asked for.
    fn no_lists(request_id: u64) -> PeerRequestResult<SnapResponse> {
        lists(request_id, &[Vec::new()], false)
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

    // An attempt at block 2 that downloaded all of `accounts()`, not handed off yet.
    fn downloaded(factory: &Factory) -> SnapWrite {
        let accounts = accounts();
        insert_chain(factory, 3, state_root(&accounts));
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(2).unwrap().unwrap().num_hash();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(pivot, state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(&accounts, 0..accounts.len(), B256::ZERO, &[]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();
        write
    }

    fn attempt_id(factory: &Factory) -> SnapAttemptId {
        factory.database_provider_ro().unwrap().snap_attempt().unwrap().unwrap().id()
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

    // Blocks `0..=tip`, each committing to an empty list and to `root`.
    fn chain(tip: u64, root: B256) -> Vec<SealedHeader> {
        let access_list_hash = compute_block_access_list_hash(&Vec::<AccountChanges>::new());
        let mut parent = B256::ZERO;
        (0..=tip)
            .map(|number| {
                let mut header = header(number, parent, Some(access_list_hash));
                header.state_root = root;
                let sealed = SealedHeader::seal_slow(header);
                parent = sealed.hash();
                sealed
            })
            .collect()
    }

    // Blocks continuing `parent`, one per list, each committing to its list and to `root`.
    fn branch(
        parent: &SealedHeader,
        lists: &[Vec<AccountChanges>],
        root: B256,
    ) -> Vec<SealedHeader> {
        let mut parent = parent.clone();
        lists
            .iter()
            .map(|list| {
                let access_list_hash = compute_block_access_list_hash(list);
                let mut header = header(parent.number() + 1, parent.hash(), Some(access_list_hash));
                header.state_root = root;
                parent = SealedHeader::seal_slow(header);
                parent.clone()
            })
            .collect()
    }

    // A peer serving `lists` in order, or none of them.
    fn lists(
        request_id: u64,
        lists: &[Vec<AccountChanges>],
        served: bool,
    ) -> PeerRequestResult<SnapResponse> {
        let block_access_lists =
            lists.iter().map(|list| served.then(|| alloy_rlp::encode(list).into())).collect();
        let message = BlockAccessListsMessage {
            request_id,
            block_access_lists: BlockAccessLists(block_access_lists),
        };
        Ok(WithPeerId::new(PeerId::random(), SnapResponse::BlockAccessLists(message)))
    }

    // Rebuilds the trie of `write`'s state up to `target` and checks it against that header.
    fn rebuild_and_verify(factory: &Factory, write: SnapWrite, target: u64) -> VerifiedSnapState {
        let provider = factory.database_provider_rw().unwrap();
        let mut stage = MerkleStage::default_execution();
        loop {
            let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap();
            let output =
                stage.execute(&provider, ExecInput { target: Some(target), checkpoint }).unwrap();
            provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint).unwrap();
            if output.done {
                break
            }
        }
        provider.verify_state_root(write).unwrap()
    }

    // The accounts at every block of the new branch, which never credits `STALE`.
    fn reorg_accounts() -> Vec<(B256, TrieAccount)> {
        let mut accounts = vec![(keccak256(STALE), account(1)), (keccak256(KEPT), account(2))];
        accounts.sort_by_key(|(hashed_address, _)| *hashed_address);
        accounts
    }

    fn stale_changes() -> AccountChanges {
        AccountChanges::new(STALE)
    }

    // An attempt that downloaded every account at pivot 4 of a branch crediting `STALE` in blocks
    // 3 and 4, then a reorg to a branch forking after block 2 whose first lists are `new_lists`.
    // Returns the orphaned headers and the new branch's.
    fn reorged(
        factory: &Factory,
        new_lists: [Vec<AccountChanges>; 2],
    ) -> (SnapAttemptId, Vec<SealedHeader>, Vec<SealedHeader>) {
        let accounts = reorg_accounts();
        let root = state_root(&accounts);
        let shared = chain(2, root);
        let orphaned = branch(&shared[2], &orphaned_lists(), root);
        insert_headers(factory, &shared);
        insert_headers(factory, &orphaned);
        let provider = factory.database_provider_rw().unwrap();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(orphaned[1].num_hash(), root)).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(&accounts, 0..2, B256::ZERO, &[]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        // The range was served at the orphaned pivot, holding its credit.
        let mut credited = Account::from(account(1));
        credited.balance = U256::from(1_000);
        let stale = HashedPostState::default().with_accounts([(keccak256(STALE), Some(credited))]);
        provider.write_hashed_state(&stale.into_sorted()).unwrap();
        provider.commit().unwrap();

        let [first, second] = new_lists;
        let new = branch(&shared[2], &[first, second, Vec::new()], root);
        factory.replace_headers_after(2, &new);
        (attempt_id(factory), orphaned, new)
    }

    // The orphaned branch's lists for blocks 3 and 4, crediting `STALE` in each.
    fn orphaned_lists() -> Vec<Vec<AccountChanges>> {
        [999, 1_000]
            .map(|balance| {
                vec![stale_changes().with_balance_change(BalanceChange::new(
                    BlockAccessIndex::new(1),
                    U256::from(balance),
                ))]
            })
            .to_vec()
    }

    // A peer serving no account range.
    fn unserved_range(request_id: u64) -> PeerRequestResult<SnapResponse> {
        let message = AccountRangeMessage { request_id, accounts: Vec::new(), proof: Vec::new() };
        Ok(WithPeerId::new(PeerId::random(), SnapResponse::AccountRange(message)))
    }

    // A peer serving the account at `hashed_address` alone.
    fn served_account(
        request_id: u64,
        accounts: &[(B256, TrieAccount)],
        hashed_address: B256,
    ) -> PeerRequestResult<SnapResponse> {
        let index = accounts.iter().position(|(key, _)| *key == hashed_address).unwrap();
        account_range(request_id, accounts, index..index + 1, &[hashed_address])
    }

    fn hashes(headers: &[SealedHeader]) -> Vec<B256> {
        headers.iter().map(SealedHeader::hash).collect()
    }

    // Runs a bootstrap through a reorg to a branch whose first lists are `new_lists`, returning
    // the account ranges it fetched again.
    async fn recover(new_lists: [Vec<AccountChanges>; 2], repaired: bool) -> Vec<B256> {
        let accounts = reorg_accounts();
        let factory = hashed_factory();
        let (attempt, orphaned, new) = reorged(&factory, new_lists.clone());
        let mut responses = vec![
            lists(1, &orphaned_lists(), true),
            // Blocks 3 and 4 of the new branch carry the state to its pivot.
            lists(2, &new_lists, true),
        ];
        if repaired {
            responses.push(served_account(1, &accounts, keccak256(STALE)));
        }
        let (client, mut bootstrap) = scripted(&factory, responses, [5]);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { write, pivot } = outcome else {
            panic!("the state is repaired: {outcome:?}")
        };
        assert_eq!(pivot, new[1].num_hash());
        assert_eq!(attempt_id(&factory), attempt);
        assert_eq!(*client.block_requests(), [hashes(&orphaned), hashes(&new[..2])]);
        let origins = client.origins().clone();
        // The repaired state is the new pivot's, as its header commits to.
        let verified = rebuild_and_verify(&factory, write, pivot.number);
        assert_eq!(verified.state_root(), state_root(&accounts));
        origins
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
    async fn pending_repairs_follow_the_pivot_once_the_accounts_are_complete() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        insert_chain(&factory, 8, root);
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(2).unwrap().unwrap().num_hash();
        let write = provider.start_snap_attempt(SnapGeneration::new(pivot, root)).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        let mut repairs = StateRepairs::default();
        repairs.insert_account(key(2));
        provider.schedule_snap_repairs(write, repairs).unwrap();
        provider.commit().unwrap();
        // No peer serves the repair at pivot 2 any more.
        let (_, mut bootstrap) = scripted(&factory, [unserved_range(1)], [3]);
        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);

        // Once the head moves on, the pivot advances and the repair is fetched there.
        let responses = [empty_lists(1, 5), account_range(1, &accounts, 1..2, &[key(2)])];
        let (client, mut bootstrap) = scripted(&factory, responses, [8]);
        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the state is complete: {outcome:?}")
        };
        assert_eq!(pivot.number, 7);
        assert_eq!(*client.origins(), [key(2)]);
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
        // One download fetches the repair and then the ranges, numbering both requests.
        let responses =
            [account_range(1, &accounts, 1..2, &[key(2)]), account_range(2, &accounts, 0..3, &[])];
        let (client, mut bootstrap) = scripted(&factory, responses, [3]);

        let outcome = bootstrap.run().await.unwrap();

        assert!(matches!(outcome, SnapBootstrapOutcome::TrieRebuild { .. }), "{outcome:?}");
        assert_eq!(*client.origins(), [key(2), B256::ZERO]);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.snap_repairs(write).unwrap().is_empty());
    }

    #[tokio::test]
    async fn repairs_yield_to_pivot_checks_between_batches() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        insert_chain(&factory, 8, root);
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(2).unwrap().unwrap().num_hash();
        let write = provider.start_snap_attempt(SnapGeneration::new(pivot, root)).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(&accounts, 0..3, B256::ZERO, &[]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        let mut repairs = StateRepairs::default();
        repairs.insert_account(key(1));
        repairs.insert_account(key(2));
        provider.schedule_snap_repairs(write, repairs).unwrap();
        provider.commit().unwrap();
        let responses = [
            account_range(1, &accounts, 0..1, &[key(1)]),
            // The head moved past the advance window meanwhile, so the pivot moves to block 7.
            empty_lists(1, 5),
            account_range(2, &accounts, 1..2, &[key(2)]),
        ];
        let (client, bootstrap) = scripted(&factory, responses, [3, 8]);
        let mut bootstrap = bootstrap.with_ranges_per_check(1);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the state is repaired: {outcome:?}")
        };
        assert_eq!(pivot.number, 7);
        assert_eq!(*client.origins(), [key(1), key(2)]);
        assert_eq!(client.block_requests().len(), 1);
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
        let verified = rebuild_and_verify(&factory, write, 7);
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
    async fn a_head_before_block_access_lists_falls_back_without_waiting() {
        let factory = hashed_factory();
        let genesis = header(0, B256::ZERO, None);
        insert_headers(&factory, &[SealedHeader::seal_slow(genesis)]);
        let (client, mut bootstrap) = scripted(&factory, [], [0]);

        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::BeforeBlockAccessLists);

        assert_eq!(bootstrap.context.waits, 0);
        assert!(client.origins().is_empty());
        assert!(factory.database_provider_ro().unwrap().snap_attempt().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_reorged_pivot_without_kept_blocks_restarts_the_attempt() {
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

    #[tokio::test]
    async fn a_new_branch_too_short_for_a_pivot_waits_before_fetching_lists() {
        let factory = hashed_factory();
        let (attempt, _, _) = reorged(&factory, [Vec::new(), Vec::new()]);
        // Head 2 puts the pivot at block 1, below the ancestor at block 2.
        let (client, mut bootstrap) = scripted(&factory, [], [2]);

        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);

        assert!(client.block_requests().is_empty());
        assert_eq!(attempt_id(&factory), attempt);
    }

    #[tokio::test]
    async fn a_handed_off_pivot_reorged_before_verification_restarts_the_attempt() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        let shared = chain(3, root);
        insert_headers(&factory, &shared);
        let (_, mut bootstrap) = scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [3]);
        let SnapBootstrapOutcome::TrieRebuild { pivot: orphaned, .. } =
            bootstrap.run().await.unwrap()
        else {
            panic!("the state is complete")
        };
        let attempt = attempt_id(&factory);
        // Blocks 2 and 3 are replaced before the rebuild verifies the orphaned pivot.
        let new = branch(&shared[1], &[vec![stale_changes()], Vec::new()], root);
        factory.replace_headers_after(1, &new);
        let (_, mut resumed) = scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [3]);

        let outcome = resumed.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { write, pivot } = outcome else {
            panic!("the new branch's state is complete: {outcome:?}")
        };
        assert_ne!(pivot, orphaned);
        assert_eq!(pivot, new[0].num_hash());
        assert_ne!(attempt_id(&factory), attempt);
        let verified = rebuild_and_verify(&factory, write, pivot.number);
        assert_eq!(verified.state_root(), root);
    }

    #[tokio::test]
    async fn a_field_only_the_orphaned_branch_changed_is_fetched_again() {
        let origins = recover([Vec::new(), Vec::new()], true).await;

        // The untouched account is kept, never fetched again.
        assert_eq!(origins, [keccak256(STALE)]);
    }

    #[tokio::test]
    async fn a_field_the_new_branch_changes_too_needs_no_repair() {
        let balance = stale_changes()
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::ONE));

        let origins = recover([vec![balance], Vec::new()], false).await;

        assert!(origins.is_empty());
    }

    #[tokio::test]
    async fn a_second_reorg_repairs_what_the_first_new_branch_changed() {
        let accounts = reorg_accounts();
        let factory = hashed_factory();
        let kept = AccountChanges::new(KEPT)
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(7)));
        let first_lists = [vec![kept], Vec::new()];
        let (attempt, _, first) = reorged(&factory, first_lists.clone());
        // Catch-up applies the first new branch, then no peer serves the repair at its pivot.
        let responses =
            [lists(1, &orphaned_lists(), true), lists(2, &first_lists, true), unserved_range(1)];
        let (_, mut bootstrap) = scripted(&factory, responses, [5]);
        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);

        // After a restart, a second reorg orphans the re-anchored pivot too.
        let shared = factory.database_provider_ro().unwrap().sealed_header(2).unwrap().unwrap();
        let second = branch(&shared, &[Vec::new(), Vec::new(), Vec::new()], state_root(&accounts));
        factory.replace_headers_after(2, &second);
        let mut responses =
            vec![lists(1, &first_lists, true), lists(2, &[Vec::new(), Vec::new()], true)];
        // Both the first orphaned branch's and the first new branch's changes are fetched again.
        let mut repaired = [keccak256(STALE), keccak256(KEPT)];
        repaired.sort();
        for (id, hashed_address) in (1..).zip(repaired) {
            responses.push(served_account(id, &accounts, hashed_address));
        }
        let (client, mut bootstrap) = scripted(&factory, responses, [5]);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { write, pivot } = outcome else {
            panic!("the state is repaired: {outcome:?}")
        };
        assert_eq!(pivot, second[1].num_hash());
        assert_eq!(attempt_id(&factory), attempt);
        assert_eq!(client.block_requests()[0], hashes(&first[..2]));
        assert_eq!(*client.origins(), repaired);
        let verified = rebuild_and_verify(&factory, write, pivot.number);
        assert_eq!(verified.state_root(), state_root(&accounts));
    }

    #[tokio::test]
    async fn unserved_orphaned_lists_are_waited_for() {
        let accounts = reorg_accounts();
        let factory = hashed_factory();
        let (attempt, orphaned, new) = reorged(&factory, [Vec::new(), Vec::new()]);
        let (_, mut bootstrap) = scripted(&factory, [lists(1, &orphaned_lists(), false)], [5]);
        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        assert_eq!(attempt_id(&factory), attempt);

        // A restarted node resumes the orphaned attempt, and another peer serves them.
        let responses = [
            lists(1, &orphaned_lists(), true),
            lists(2, &[Vec::new(), Vec::new()], true),
            served_account(1, &accounts, keccak256(STALE)),
        ];
        let (client, mut bootstrap) = scripted(&factory, responses, [5]);
        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the state is repaired: {outcome:?}")
        };
        assert_eq!(pivot, new[1].num_hash());
        assert_eq!(attempt_id(&factory), attempt);
        assert_eq!(client.block_requests()[0], hashes(&orphaned));
    }

    #[tokio::test]
    async fn a_reorg_across_block_access_list_activation_restarts_the_attempt() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        let mut shared = chain(1, root);
        // The ancestor predates block access lists, so the branches may hold blocks without them.
        let mut ancestor = header(2, shared[1].hash(), None);
        ancestor.state_root = root;
        shared.push(SealedHeader::seal_slow(ancestor));
        let orphaned = branch(&shared[2], &orphaned_lists(), root);
        insert_headers(&factory, &shared);
        insert_headers(&factory, &orphaned);
        let provider = factory.database_provider_rw().unwrap();
        provider.start_snap_attempt(SnapGeneration::new(orphaned[1].num_hash(), root)).unwrap();
        provider.commit().unwrap();
        let attempt = attempt_id(&factory);
        let new = branch(&shared[2], &[vec![stale_changes()], Vec::new(), Vec::new()], root);
        factory.replace_headers_after(2, &new);
        let (client, mut bootstrap) =
            scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [5]);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the new branch's state is complete: {outcome:?}")
        };
        assert_eq!(pivot, new[1].num_hash());
        assert_ne!(attempt_id(&factory), attempt);
        assert!(client.block_requests().is_empty());
    }

    #[tokio::test]
    async fn a_catch_up_stalled_below_the_ancestor_restarts_before_fetching_lists() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        let shared = chain(2, root);
        let orphaned = branch(&shared[2], &orphaned_lists(), root);
        insert_headers(&factory, &shared);
        insert_headers(&factory, &orphaned);
        // The pivot advanced to block 4 while catch-up stayed at block 1, below the ancestor.
        let provider = factory.database_provider_rw().unwrap();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(shared[1].num_hash(), root)).unwrap();
        provider
            .advance_snap_pivot(write, SnapGeneration::new(orphaned[1].num_hash(), root))
            .unwrap();
        provider.commit().unwrap();
        let attempt = attempt_id(&factory);
        let mut lists = vec![vec![stale_changes()]];
        lists.resize(8, Vec::new());
        let new = branch(&shared[2], &lists, root);
        factory.replace_headers_after(2, &new);
        // Head 10 still serves the list after the ancestor, but not the one after block 1.
        let (client, mut bootstrap) =
            scripted(&factory, [account_range(1, &accounts, 0..3, &[])], [10]);

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the new branch's state is complete: {outcome:?}")
        };
        assert_eq!(pivot, new[6].num_hash());
        assert_ne!(attempt_id(&factory), attempt);
        assert!(client.block_requests().is_empty());
    }

    #[tokio::test]
    async fn orphaned_lists_unserved_past_the_state_window_restart_the_attempt() {
        let accounts = accounts();
        let root = state_root(&accounts);
        let factory = hashed_factory();
        let shared = chain(2, root);
        let orphaned = branch(&shared[2], &orphaned_lists(), root);
        insert_headers(&factory, &shared);
        insert_headers(&factory, &orphaned);
        let provider = factory.database_provider_rw().unwrap();
        provider.start_snap_attempt(SnapGeneration::new(orphaned[1].num_hash(), root)).unwrap();
        provider.commit().unwrap();
        let attempt = attempt_id(&factory);
        let mut new_lists = vec![vec![stale_changes()]];
        new_lists.resize(129, Vec::new());
        let new = branch(&shared[2], &new_lists, root);
        factory.replace_headers_after(2, &new);
        // Head 131 leaves the ancestor past the served state window, but its lists are still
        // served.
        let responses =
            [lists(1, &orphaned_lists(), false), account_range(1, &accounts, 0..3, &[])];
        let (_, bootstrap) = scripted(&factory, responses, [131]);
        let mut bootstrap = bootstrap.with_policy(policy().with_history(256));

        let outcome = bootstrap.run().await.unwrap();

        let SnapBootstrapOutcome::TrieRebuild { pivot, .. } = outcome else {
            panic!("the new branch's state is complete: {outcome:?}")
        };
        assert_eq!(pivot, new[127].num_hash());
        assert_ne!(attempt_id(&factory), attempt);
    }

    #[tokio::test]
    async fn a_cancelled_repair_resumes_at_its_pending_slots() {
        let slots = vec![(key(1), U256::from(11)), (key(2), U256::from(12))];
        let mut contract = account(1);
        contract.storage_root = storage_root_of(&slots);
        let accounts = vec![(key(1), contract)];
        let factory = hashed_factory();
        insert_chain(&factory, 3, state_root(&accounts));
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(2).unwrap().unwrap().num_hash();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(pivot, state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let mut repairs = StateRepairs::default();
        repairs.insert_slot(key(1), key(1));
        repairs.insert_slot(key(1), key(2));
        provider.schedule_snap_repairs(write, repairs).unwrap();
        provider.commit().unwrap();
        let cancel = CancellationToken::new();
        let on_request = cancel.clone();
        // Cancellation is checked between repair batches, so the batch ends first, with
        // `key(2)` left pending by a peer that does not serve it.
        let client = Arc::new(
            ScriptedSnapClient::new([
                account_range(1, &accounts, 0..1, &[key(1)]),
                storage_ranges(1, &[&slots[..1]], &slots, &[key(1)]),
                storage_ranges(2, &[], &[], &[]),
            ])
            .on_storage_request(move || on_request.cancel()),
        );
        let context = TestContext { heads: RefCell::new(VecDeque::from([3])), waits: 0 };
        let mut bootstrap =
            SnapBootstrap::new(Arc::clone(&client), factory.clone(), Runtime::test(), context)
                .with_policy(policy())
                .with_cancellation(cancel);
        assert_eq!(bootstrap.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        assert_eq!(client.storage_requests().len(), 2);
        let provider = factory.database_provider_ro().unwrap();
        let pending = provider.snap_repairs(write).unwrap().slots(key(1)).collect::<Vec<_>>();
        assert_eq!(pending, [key(2)]);
        assert_eq!(stored_slots(&provider, key(1)), slots[..1]);
        drop(provider);

        let (client, mut resumed) = scripted(
            &factory,
            [
                account_range(1, &accounts, 0..1, &[key(1)]),
                storage_ranges(1, &[&slots[1..]], &slots, &[key(2)]),
                Err(RequestError::UnsupportedCapability),
            ],
            [3],
        );
        assert_eq!(resumed.run().await.unwrap(), SnapBootstrapOutcome::Stopped);
        assert_eq!(*client.storage_requests(), [(vec![key(1)], key(2))]);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.snap_repairs(write).unwrap().is_empty());
        assert_eq!(stored_slots(&provider, key(1)), slots);
    }

    #[tokio::test]
    async fn a_cancelled_run_still_finishes_the_hand_off_scan() {
        let factory = hashed_factory();
        let write = downloaded(&factory);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (_, bootstrap) = scripted(&factory, [], [3]);
        let bootstrap = bootstrap.with_cancellation(cancel).with_shutdown(CancellationToken::new());

        // The scan restarts from scratch once stopped, so a header refresh doesn't stop it.
        assert!(matches!(bootstrap.hand_off(write).await, Ok(Step::HandedOff(_))));

        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.is_trie_rebuild_started(write).unwrap());
    }

    #[tokio::test]
    async fn shutdown_stops_the_hand_off_scan_without_committing() {
        let factory = hashed_factory();
        let write = downloaded(&factory);
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let (_, bootstrap) = scripted(&factory, [], [3]);
        let bootstrap = bootstrap.with_shutdown(shutdown);

        assert!(matches!(bootstrap.hand_off(write).await, Err(SnapSyncError::Cancelled)));

        let provider = factory.database_provider_ro().unwrap();
        assert!(!provider.is_trie_rebuild_started(write).unwrap());
    }
}
