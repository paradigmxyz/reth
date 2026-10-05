//! Hands snap-downloaded state over to the staged pipeline.
//!
//! The merkle stage rebuilds the trie from the downloaded state, and only a matching root on a
//! still canonical pivot is published and accepted.

use alloy_eips::BlockNumHash;
use reth_errors::{ConsensusError, RethError};
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, HeaderProvider,
    ProviderFactory, ProviderResult, StageCheckpointReader, StageCheckpointWriter,
};
use reth_snap_sync::{SnapAttemptStore, SnapStateVerifier, SnapWrite};
use reth_stages::{
    stages::MerkleStage, BlockErrorKind, ExecInput, PipelineError, Stage, StageError, StageId,
};
use reth_tracing::tracing::{info, warn};
use tokio_util::sync::CancellationToken;

/// Hands one attempt's downloaded state over to the staged pipeline.
///
/// Every step commits on its own and repeats safely, so an interrupted handoff runs again.
#[derive(Debug)]
pub struct SnapHandoff<N: ProviderNodeTypes> {
    // Database the state was downloaded into.
    factory: ProviderFactory<N>,
}

impl<N: ProviderNodeTypes> SnapHandoff<N> {
    /// Creates a handoff over the node's database.
    pub const fn new(factory: ProviderFactory<N>) -> Self {
        Self { factory }
    }

    /// Rebuilds the trie at the pivot of `write`'s attempt and checks its root, abandoning the
    /// attempt on a mismatch so the next run starts a new one. Progress commits as it goes, and
    /// `stop` ends it early.
    ///
    /// The attempt's state must have been handed to the merkle stage with
    /// [`SnapStateVerifier::start_trie_rebuild`].
    pub fn rebuild(
        &self,
        write: SnapWrite,
        stop: &CancellationToken,
    ) -> Result<RebuildOutcome, PipelineError> {
        let pivot = self.pivot(write)?;
        match self.rebuild_trie(pivot, stop) {
            Err(PipelineError::Stage(StageError::Block {
                error: BlockErrorKind::Validation(ConsensusError::BodyStateRootDiff(diff)),
                ..
            })) => {
                warn!(target: "sync::snap", ?pivot, %diff, "Snap state root mismatch, abandoning the attempt");
                self.abandon()?;
                Ok(RebuildOutcome::RootMismatch)
            }
            rebuilt => rebuilt,
        }
    }

    /// Publishes the state downloaded under `write` at its attempt's pivot and accepts it, so the
    /// pipeline continues above the pivot. The pivot must still be canonical, and its trie is
    /// rebuilt first if [`Self::rebuild`] hasn't finished, until `stop` fires.
    ///
    /// The attempt's state must have been handed to the merkle stage with
    /// [`SnapStateVerifier::start_trie_rebuild`].
    pub fn hand_off(
        &self,
        write: SnapWrite,
        stop: &CancellationToken,
    ) -> Result<HandoffOutcome, PipelineError> {
        let pivot = self.pivot(write)?;
        if !self.is_canonical(pivot)? {
            self.abandon()?;
            return Ok(HandoffOutcome::PivotReorged)
        }

        // A trie already rebuilt to the pivot returns at once.
        match self.rebuild(write, stop)? {
            RebuildOutcome::Rebuilt => {}
            RebuildOutcome::RootMismatch => return Ok(HandoffOutcome::RootMismatch),
            RebuildOutcome::Stopped => return Ok(HandoffOutcome::Stopped),
        }

        self.publish(write)?;
        info!(target: "sync::snap", pivot = pivot.number, "Snap state published; history below it is unavailable");
        Ok(HandoffOutcome::Completed)
    }

    /// Publishes an attempt whose trie is rebuilt at a still canonical pivot, finishing a publish
    /// that stopped before its database commit. An attempt rebuilt but not yet handed off is
    /// published as well. Runs before the consistency check, which would otherwise unwind the
    /// static files below their anchor.
    pub fn resume_interrupted_publish(&self) -> Result<(), PipelineError> {
        let Some((write, pivot)) = self.interrupted_publish()? else { return Ok(()) };
        info!(target: "sync::snap", pivot, "Resuming an interrupted snap state publish");
        self.publish(write)
    }

    // An unfinished attempt whose trie is rebuilt at a still canonical pivot. Read from the
    // database alone, since a publish deletes the static files before that commit.
    fn interrupted_publish(&self) -> Result<Option<(SnapWrite, u64)>, PipelineError> {
        let provider = self.factory.provider()?;
        let Some(write) = provider.active_snap_write().map_err(RethError::other)? else {
            return Ok(None)
        };
        let pivot = provider.authorize_snap_write(write).map_err(RethError::other)?.pivot();
        let rebuilt = provider.is_trie_rebuild_started(write).map_err(RethError::other)? &&
            provider
                .get_stage_checkpoint(StageId::MerkleExecute)?
                .is_some_and(|checkpoint| checkpoint.block_number == pivot.number);
        Ok((rebuilt && self.is_canonical(pivot)?).then_some((write, pivot.number)))
    }

    // Pivot of the attempt `write` belongs to, refusing a write the attempt no longer accepts.
    fn pivot(&self, write: SnapWrite) -> Result<BlockNumHash, PipelineError> {
        let provider = self.factory.provider()?;
        Ok(provider.authorize_snap_write(write).map_err(RethError::other)?.pivot())
    }

    // Forkchoice can reorg the pivot out while its state downloads, and publishing anchors the
    // node to it, so a pivot that is no longer canonical is dropped instead of published.
    fn is_canonical(&self, pivot: BlockNumHash) -> ProviderResult<bool> {
        Ok(self
            .factory
            .provider()?
            .sealed_header(pivot.number)?
            .is_some_and(|header| header.hash() == pivot.hash))
    }

    // Drops the attempt, so the next run starts a new one.
    fn abandon(&self) -> Result<(), PipelineError> {
        let provider = self.factory.database_provider_rw()?;
        provider.abandon_snap_attempt().map_err(RethError::other)?;
        provider.commit()?;
        Ok(())
    }

    // Accepts the state, anchors the static files at the verified pivot and moves the checkpoints
    // there in one database commit, so a stop before it leaves the attempt to resume.
    fn publish(&self, write: SnapWrite) -> Result<(), PipelineError> {
        let provider = self.factory.database_provider_rw()?;
        // Deleted static files don't roll back with the database, so a rejected state must fail
        // before them.
        let pivot = provider.verify_state_root(write).map_err(RethError::other)?.target().number;
        // History below the pivot was never downloaded, so it counts as pruned and the static
        // files start there.
        provider.anchor_pruned_static_files(pivot)?;
        provider.publish_snap_state(pivot).map_err(RethError::other)?;
        provider.commit()?;
        Ok(())
    }

    // Rebuilds the trie from the downloaded state up to `pivot`, committing the stage's progress
    // in chunks so a restart resumes it. The stage checks the root against the pivot's header.
    fn rebuild_trie(
        &self,
        pivot: BlockNumHash,
        stop: &CancellationToken,
    ) -> Result<RebuildOutcome, PipelineError> {
        let mut stage = MerkleStage::default_execution();
        loop {
            if stop.is_cancelled() {
                return Ok(RebuildOutcome::Stopped)
            }
            let provider = self.factory.database_provider_rw()?;
            let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute)?;
            let output =
                stage.execute(&provider, ExecInput { target: Some(pivot.number), checkpoint })?;
            provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint)?;
            provider.commit()?;
            if output.done {
                return Ok(RebuildOutcome::Rebuilt)
            }
        }
    }
}

/// What a handoff did with the downloaded state.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub enum HandoffOutcome {
    /// The state is published, its trie rebuilt and its root verified.
    Completed,
    /// The pivot left the canonical chain, so the attempt was abandoned for a new one.
    PivotReorged,
    /// The rebuilt root differs from the pivot's header, so the attempt was abandoned for a new
    /// one.
    RootMismatch,
    /// The trie rebuild was stopped before reaching the pivot, so nothing was published.
    Stopped,
}

/// How a trie rebuild ended.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub enum RebuildOutcome {
    /// The trie is rebuilt at the pivot and its root matches the pivot's header.
    Rebuilt,
    /// The rebuilt root differs from the pivot's header, so the attempt was abandoned for a new
    /// one.
    RootMismatch,
    /// The rebuild was stopped before reaching the pivot. Its progress so far is committed.
    Stopped,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::snap::tests::hashed_factory;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use futures::future::{ready, Ready};
    use reth_db::{tables, transaction::DbTxMut};
    use reth_downloaders::snap::{AccountRangeDownloader, AccountRangeOutcome};
    use reth_eth_wire_types::snap::{
        AccountData, AccountRangeMessage, GetAccountRangeMessage, GetBlockAccessListsMessage,
        GetByteCodesMessage, GetStorageRangesMessage,
    };
    use reth_network_p2p::{
        download::DownloadClient,
        error::PeerRequestResult,
        priority::Priority,
        snap::client::{SnapClient, SnapResponse},
    };
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, BlockWriter, MetadataProvider, StaticFileProviderFactory,
        StaticFileSegment, StaticFileWriter,
    };
    use reth_snap_sync::{
        SnapAccountStore, SnapGeneration, DEFAULT_RESPONSE_BYTES, DEFAULT_SCAN_CHUNK, MAX_HASH,
    };
    use reth_stages::StageCheckpoint;
    use reth_tasks::Runtime;
    use reth_trie_common::{root::state_root_unsorted, TrieAccount};

    pub(crate) const PIVOT: u64 = 1;

    // Serves every account range from one trie, as a peer holding all of it would. A complete
    // trie needs no proof.
    #[derive(Debug)]
    struct WholeTrieClient(Vec<(B256, TrieAccount)>);

    impl DownloadClient for WholeTrieClient {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl SnapClient for WholeTrieClient {
        type Output = Ready<PeerRequestResult<SnapResponse>>;

        fn get_account_range_with_priority(
            &self,
            request: GetAccountRangeMessage,
            _priority: Priority,
        ) -> Self::Output {
            let accounts = self
                .0
                .iter()
                .map(|(key, account)| AccountData::from_trie_account(*key, account))
                .collect();
            let message =
                AccountRangeMessage { request_id: request.request_id, accounts, proof: Vec::new() };
            ready(Ok(WithPeerId::new(PeerId::random(), SnapResponse::AccountRange(message))))
        }

        fn get_storage_ranges(&self, _request: GetStorageRangesMessage) -> Self::Output {
            unreachable!("the accounts have no storage")
        }

        fn get_storage_ranges_with_priority(
            &self,
            _request: GetStorageRangesMessage,
            _priority: Priority,
        ) -> Self::Output {
            unreachable!("the accounts have no storage")
        }

        fn get_byte_codes(&self, _request: GetByteCodesMessage) -> Self::Output {
            unreachable!("the accounts have no code")
        }

        fn get_byte_codes_with_priority(
            &self,
            _request: GetByteCodesMessage,
            _priority: Priority,
        ) -> Self::Output {
            unreachable!("the accounts have no code")
        }

        fn get_block_access_lists_with_priority(
            &self,
            _request: GetBlockAccessListsMessage,
            _priority: Priority,
        ) -> Self::Output {
            unreachable!("the attempt stays at its pivot")
        }
    }

    // Headers through the pivot on storage v2.
    fn with_headers() -> ProviderFactory<MockNodeTypesWithDB> {
        with_headers_committing_to(Header::default().state_root)
    }

    // Headers through the pivot on storage v2, each committing to `state_root`.
    fn with_headers_committing_to(state_root: B256) -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = hashed_factory();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut parent = B256::ZERO;
        for number in 0..=PIVOT {
            let header = Header { number, parent_hash: parent, state_root, ..Default::default() };
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
            parent = hash;
        }
        writer.commit().unwrap();
        drop(writer);
        factory
    }

    // Headers through the pivot, with an unverified attempt downloading state at it.
    fn downloading() -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite) {
        let factory = with_headers();
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(PIVOT).unwrap().unwrap().num_hash();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(pivot, B256::repeat_byte(1))).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    // Headers committing to two plain accounts, with an attempt at the pivot that downloaded both.
    pub(crate) fn downloaded_attempt() -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite) {
        let accounts: Vec<_> = (1..=2)
            .map(|nonce| {
                (
                    B256::with_last_byte(nonce),
                    TrieAccount { nonce: nonce.into(), ..Default::default() },
                )
            })
            .collect();
        let root = state_root_unsorted(accounts.clone());
        let factory = with_headers_committing_to(root);
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(PIVOT).unwrap().unwrap().num_hash();
        let write = provider.start_snap_attempt(SnapGeneration::new(pivot, root)).unwrap();
        provider.start_account_coverage(write).unwrap();
        let request = GetAccountRangeMessage {
            request_id: 1,
            root_hash: root,
            starting_hash: B256::ZERO,
            limit_hash: MAX_HASH,
            response_bytes: DEFAULT_RESPONSE_BYTES,
        };
        let downloader =
            AccountRangeDownloader::new(WholeTrieClient(accounts), request, Runtime::test())
                .unwrap();
        let AccountRangeOutcome::Verified(range) = futures::executor::block_on(downloader).unwrap()
        else {
            panic!("the client serves the requested root")
        };
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    // An attempt that downloaded all of its state, handed to the merkle stage.
    fn handed_to_merkle() -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite) {
        let (factory, write) = downloaded_attempt();
        let provider = factory.database_provider_rw().unwrap();
        provider.start_trie_rebuild(write, DEFAULT_SCAN_CHUNK, &CancellationToken::new()).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    // An attempt that downloaded all of its state, with its trie rebuilt at the pivot.
    fn rebuilt() -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite) {
        let (factory, write) = handed_to_merkle();
        let rebuild = SnapHandoff::new(factory.clone()).rebuild(write, &CancellationToken::new());
        assert_eq!(rebuild.unwrap(), RebuildOutcome::Rebuilt);
        (factory, write)
    }

    // A publish whose static files committed before the node stopped, ahead of its database
    // commit.
    fn interrupt_publish(factory: &ProviderFactory<MockNodeTypesWithDB>) {
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.publish_snap_state(PIVOT).unwrap();
        factory.static_file_provider().finalize().unwrap();
    }

    // Asserts the state is published at the pivot and accepted.
    fn assert_published(factory: &ProviderFactory<MockNodeTypesWithDB>) {
        assert_eq!(factory.check_consistency().unwrap(), (None, None));
        let static_files = factory.static_file_provider();
        for segment in StaticFileSegment::iter().filter(|segment| !segment.is_headers()) {
            assert_eq!(
                static_files.get_highest_static_file_block(segment),
                Some(PIVOT),
                "{segment}"
            );
        }
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::Execution).unwrap(),
            Some(StageCheckpoint::new(PIVOT))
        );
        assert!(provider.snap_attempt().unwrap().unwrap().is_verified());
    }

    // Headers through the pivot, with the state published at it.
    fn published() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = with_headers();
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.publish_snap_state(PIVOT).unwrap();
        provider.commit().unwrap();
        factory
    }

    #[test]
    fn resuming_without_an_interrupted_publish_changes_nothing() {
        let (factory, ..) = downloading();

        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
        assert_eq!(factory.check_consistency().unwrap(), (None, None));
    }

    #[test]
    fn the_block_after_the_pivot_appends_to_the_anchored_files() {
        let factory = published();
        let provider = factory.database_provider_rw().unwrap();

        // Appending fails unless the anchored segments start right after the pivot.
        provider.append_block_bodies(vec![(PIVOT + 1, Some(&Default::default()))]).unwrap();
        provider.commit().unwrap();
    }

    #[test]
    fn a_pivot_reorged_out_is_abandoned_instead_of_published() {
        let factory = with_headers();
        let provider = factory.database_provider_rw().unwrap();
        let orphan = BlockNumHash::new(PIVOT, B256::repeat_byte(0xaa));
        let write = provider
            .start_snap_attempt(SnapGeneration::new(orphan, B256::repeat_byte(0xbb)))
            .unwrap();
        provider.commit().unwrap();

        // The canonical header at the pivot's number is a different block now.
        let handoff =
            SnapHandoff::new(factory.clone()).hand_off(write, &CancellationToken::new()).unwrap();

        assert_eq!(handoff, HandoffOutcome::PivotReorged);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.active_snap_write().unwrap().is_none());
    }

    #[test]
    fn a_stopped_rebuild_leaves_the_trie_unbuilt() {
        let (factory, write) = downloading();
        let stop = CancellationToken::new();
        stop.cancel();

        let rebuild = SnapHandoff::new(factory.clone()).rebuild(write, &stop).unwrap();

        assert_eq!(rebuild, RebuildOutcome::Stopped);

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap(), None);
    }

    #[test]
    fn an_abandoned_attempt_is_not_an_interrupted_publish() {
        let (factory, ..) = downloading();
        let provider = factory.database_provider_rw().unwrap();
        provider.abandon_snap_attempt().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.commit().unwrap();

        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
    }

    #[test]
    fn a_root_mismatch_abandons_the_attempt_before_publishing() {
        let (factory, write) = downloading();
        let provider = factory.database_provider_rw().unwrap();
        // State whose root differs from the empty root the pivot header commits to.
        provider
            .tx_ref()
            .put::<tables::HashedAccounts>(
                B256::repeat_byte(3),
                Account { nonce: 1, ..Default::default() },
            )
            .unwrap();
        provider.commit().unwrap();

        let handoff = SnapHandoff::new(factory.clone()).hand_off(write, &CancellationToken::new());

        assert_eq!(handoff.unwrap(), HandoffOutcome::RootMismatch);

        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.active_snap_write().unwrap().is_none());
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
        assert_eq!(
            factory.static_file_provider().get_lowest_range_start(StaticFileSegment::Transactions),
            None
        );
    }

    #[test]
    fn a_handoff_accepts_the_state_with_its_publish() {
        let (factory, write) = rebuilt();

        let handoff =
            SnapHandoff::new(factory.clone()).hand_off(write, &CancellationToken::new()).unwrap();

        assert_eq!(handoff, HandoffOutcome::Completed);
        assert_published(&factory);
    }

    #[test]
    fn a_publish_interrupted_before_its_checkpoints_resumes_at_startup() {
        let (factory, ..) = rebuilt();
        interrupt_publish(&factory);

        // Startup finishes the publish before checking consistency, which would otherwise unwind
        // the anchored files to the old checkpoints.
        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        assert_published(&factory);
    }

    #[test]
    fn a_rejected_publish_keeps_the_static_files() {
        let (factory, write) = rebuilt();
        let provider = factory.database_provider_rw().unwrap();
        provider.abandon_snap_attempt().unwrap();
        provider.commit().unwrap();

        assert!(SnapHandoff::new(factory.clone()).publish(write).is_err());

        assert_eq!(factory.static_file_provider().earliest_history_height(), 0);
    }

    #[test]
    fn a_resume_cut_short_resumes_again() {
        let (factory, ..) = rebuilt();
        interrupt_publish(&factory);

        // The resume deletes the anchored files at once, then stops before anything commits.
        factory.database_provider_rw().unwrap().anchor_pruned_static_files(PIVOT).unwrap();

        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        assert_published(&factory);
    }

    #[test]
    fn a_stopped_handoff_publishes_nothing() {
        let (factory, write) = handed_to_merkle();
        let stop = CancellationToken::new();
        stop.cancel();

        let handoff = SnapHandoff::new(factory.clone()).hand_off(write, &stop).unwrap();

        assert_eq!(handoff, HandoffOutcome::Stopped);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.active_snap_write().unwrap(), Some(write));
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
        assert_eq!(factory.static_file_provider().earliest_history_height(), 0);
    }
}
