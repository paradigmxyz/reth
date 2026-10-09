//! Decides when downloaded state is complete, and accepts it once its trie matches the pivot.
//!
//! The trie itself is rebuilt by the merkle stage, which commits its progress in chunks and checks
//! the root against the target header, so a rebuild survives restarts on any state size.

use crate::{
    common::SnapRecord, SnapAccountStore, SnapAttemptStore, SnapCatchUpStore, SnapSyncError,
    SnapWrite,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{B256, KECCAK256_EMPTY};
use reth_db_api::{
    cursor::DbCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable,
};
use reth_primitives_traits::{AlloyBlockHeader, GotExpected};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockHashReader, BlockWriter, DBProvider, HeaderProvider, MetadataProvider, MetadataWriter,
    PruneCheckpointWriter, SnapAttemptId, StageCheckpointReader, StageCheckpointWriter,
};
use reth_storage_errors::provider::{ProviderError, RootMismatch};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Accounts scanned between cancellation checks, bounding how long a cancelled session keeps
/// running.
pub const DEFAULT_SCAN_CHUNK: u64 = 100_000;

// Stages publishing moves to the pivot, since the downloaded state replaces their output. Headers
// and era import run on their own, the merkle stage still rebuilds the trie, and `Finish` waits
// for verification.
const PUBLISHED_STAGES: [StageId; 11] = [
    StageId::Bodies,
    StageId::SenderRecovery,
    StageId::Execution,
    StageId::PruneSenderRecovery,
    StageId::MerkleUnwind,
    StageId::AccountHashing,
    StageId::StorageHashing,
    StageId::TransactionLookup,
    StageId::IndexStorageHistory,
    StageId::IndexAccountHistory,
    StageId::Prune,
];

/// Decides whether downloaded state can be trusted as the node's state.
///
/// Verification writes join the caller's transaction, so a refused check changes nothing.
pub trait SnapStateVerifier {
    /// Hands complete state to the merkle stage, which rebuilds its trie from scratch.
    ///
    /// Progress the stage recorded before the hand-off describes other state, so it is discarded.
    /// The hand-off is recorded for this attempt and pivot, so only a rebuild of this state counts.
    fn start_trie_rebuild(
        &self,
        write: SnapWrite,
        chunk: u64,
        cancel: &CancellationToken,
    ) -> Result<(), SnapSyncError>
    where
        Self: MetadataWriter + StageCheckpointWriter + DBProvider;

    /// Refuses to finish while any downloaded state is still pending. Storage commits with its
    /// accounts, so it needs no separate check.
    fn verify_completeness(
        &self,
        write: SnapWrite,
        chunk: u64,
        cancel: &CancellationToken,
    ) -> Result<(), SnapSyncError>
    where
        Self: DBProvider;

    /// Publishes the state downloaded at `pivot`: makes it the node's state at that block, so the
    /// pipeline resumes at `pivot + 1` instead of executing from genesis.
    ///
    /// - Moves the stages the downloaded state covers to `pivot`.
    /// - Records history below `pivot` as pruned.
    /// - Clears body indices, database receipts and transaction lookups, so transaction numbers
    ///   restart at 0 above `pivot`. `RocksDB` lookups are cleared immediately, not on commit.
    ///
    /// Publishing is not verification: the merkle stage and `Finish` wait for the trie rebuild.
    /// It cannot be undone, the node must not unwind below `pivot`, and the caller resets the
    /// static files to `pivot` in the same commit.
    fn publish_snap_state(&self, pivot: u64) -> Result<(), SnapSyncError>
    where
        Self: BlockWriter + PruneCheckpointWriter + StageCheckpointWriter + DBProvider<Tx: DbTxMut>;

    /// Returns whether `write`'s state was handed to the merkle stage, so a resumed sync does not
    /// reset its rebuild.
    fn is_trie_rebuild_started(&self, write: SnapWrite) -> Result<bool, SnapSyncError>;

    /// Accepts the state once the merkle stage has rebuilt its trie up to the pivot.
    ///
    /// The stage checks the root against the pivot header, which must commit to the root the
    /// state was downloaded against.
    fn verify_state_root(&self, write: SnapWrite) -> Result<VerifiedSnapState, SnapSyncError>
    where
        Self: BlockHashReader + HeaderProvider + MetadataWriter + StageCheckpointReader;
}

/// Downloaded state whose trie root matched the header of the block it is anchored to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedSnapState {
    // Attempt that downloaded the state.
    attempt: SnapAttemptId,
    // Block whose header commits to the state.
    target: BlockNumHash,
    // Root the target header commits to.
    state_root: B256,
}

impl VerifiedSnapState {
    /// Attempt that downloaded the state.
    pub const fn attempt(&self) -> SnapAttemptId {
        self.attempt
    }

    /// Block whose header commits to the state.
    pub const fn target(&self) -> BlockNumHash {
        self.target
    }

    /// Root the target header commits to.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }
}

impl<T: MetadataProvider> SnapStateVerifier for T {
    fn start_trie_rebuild(
        &self,
        write: SnapWrite,
        chunk: u64,
        cancel: &CancellationToken,
    ) -> Result<(), SnapSyncError>
    where
        Self: MetadataWriter + StageCheckpointWriter + DBProvider,
    {
        // The stage treats a checkpoint at genesis as already rebuilt, so it would never check
        // state anchored there.
        if self.authorize_snap_write(write)?.pivot().number == 0 {
            return Err(SnapSyncError::GenesisPivot)
        }
        self.verify_completeness(write, chunk, cancel)?;
        // Starting from block zero makes the stage clear the trie tables and rebuild them, since
        // downloaded state has no changesets to update an existing trie from.
        self.save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::default())?;
        self.save_stage_checkpoint_progress(StageId::MerkleExecute, Vec::new())?;
        StoredRebuild::new(write).write(self)?;
        Ok(())
    }

    fn publish_snap_state(&self, pivot: u64) -> Result<(), SnapSyncError>
    where
        Self: BlockWriter + PruneCheckpointWriter + StageCheckpointWriter + DBProvider<Tx: DbTxMut>,
    {
        if pivot == 0 {
            return Err(SnapSyncError::GenesisPivot)
        }
        // Bodies downloaded before the pivot moved allocated transaction numbers that the emptied
        // transaction segments no longer hold.
        self.tx_ref().clear::<tables::BlockBodyIndices>()?;
        self.tx_ref().clear::<tables::TransactionBlocks>()?;
        // Their withdrawals and ommers are appended per block, so writing above the pivot again
        // would fail on the rows left behind. These are the Ethereum body tables, a chain storage
        // with body tables of its own has to clear them as well.
        self.tx_ref().clear::<tables::BlockWithdrawals>()?;
        self.tx_ref().clear::<tables::BlockOmmers>()?;
        // Receipt log filtering keeps receipts in MDBX even with storage v2. Their transaction
        // numbers must be reusable after publication.
        self.tx_ref().clear::<tables::Receipts>()?;
        self.clear_transaction_lookup()?;

        let checkpoint = StageCheckpoint::new(pivot);
        for stage in PUBLISHED_STAGES {
            self.save_stage_checkpoint(stage, checkpoint)?;
        }
        // Snap sync wrote no history below the pivot.
        let pruned = PruneCheckpoint::pruned_through(pivot);
        for segment in PruneSegment::variants() {
            self.save_prune_checkpoint(segment, pruned)?;
        }
        Ok(())
    }

    fn verify_completeness(
        &self,
        write: SnapWrite,
        chunk: u64,
        cancel: &CancellationToken,
    ) -> Result<(), SnapSyncError>
    where
        Self: DBProvider,
    {
        let attempt = self.authorize_snap_write(write)?;
        let coverage = self.account_coverage(write)?.ok_or(SnapSyncError::NoCoverage)?;
        if let Some(next) = coverage.next() {
            return Err(SnapSyncError::IncompleteAccounts { next })
        }
        let applied =
            self.catch_up_progress(write)?.ok_or(SnapSyncError::NoCatchUpProgress)?.applied();
        if applied != attempt.pivot() {
            return Err(SnapSyncError::CatchUpBehindPivot {
                applied: applied.number,
                pivot: attempt.pivot().number,
            })
        }
        let repairs = self.snap_repairs(write)?;
        if !repairs.is_empty() {
            return Err(SnapSyncError::PendingRepairs { accounts: repairs.len() })
        }
        ensure_code_present(self.tx_ref(), chunk, cancel)
    }

    fn is_trie_rebuild_started(&self, write: SnapWrite) -> Result<bool, SnapSyncError> {
        Ok(StoredRebuild::read(self)?.is_some_and(|stored| stored.write == write))
    }

    fn verify_state_root(&self, write: SnapWrite) -> Result<VerifiedSnapState, SnapSyncError>
    where
        Self: BlockHashReader + HeaderProvider + MetadataWriter + StageCheckpointReader,
    {
        let attempt = self.authorize_canonical_snap_write(write)?;
        let target = attempt.pivot();
        let header = self
            .sealed_header(target.number)?
            .filter(|header| header.hash() == target.hash)
            .ok_or(SnapSyncError::MissingHeader { block: target.number })?;
        // The stage answers to the header, while downloaded ranges answered to the attempt.
        if header.state_root() != attempt.state_root() {
            return Err(ProviderError::StateRootMismatch(Box::new(RootMismatch {
                root: GotExpected { got: attempt.state_root(), expected: header.state_root() },
                block_number: target.number,
                block_hash: target.hash,
            }))
            .into())
        }
        // Until the stage reaches the pivot after this state's hand-off, the trie holds no state
        // for it: an earlier attempt's rebuild can end at the same block.
        let handed_off = self.is_trie_rebuild_started(write)?;
        let rebuilt = self.get_stage_checkpoint(StageId::MerkleExecute)?;
        if !handed_off || rebuilt.map(|checkpoint| checkpoint.block_number) != Some(target.number) {
            return Err(ProviderError::StateForNumberNotFound(target.number).into())
        }
        self.verify_snap_attempt(write)?;
        Ok(VerifiedSnapState { attempt: attempt.id(), target, state_root: header.state_root() })
    }
}

// The trie rebuild hand-off as persisted, tied to the write it was made for.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredRebuild {
    // Encoding version, checked before the rest is decoded.
    version: u32,
    // Attempt and pivot the state was handed off at.
    write: SnapWrite,
}

impl SnapRecord for StoredRebuild {
    const KEY: &'static str = "snap_trie_rebuild";
    const VERSION: u32 = 1;
}

impl StoredRebuild {
    // The hand-off of `write`'s state at this build's version.
    const fn new(write: SnapWrite) -> Self {
        Self { version: Self::VERSION, write }
    }
}

// Refuses the first account whose code is not stored, checking for cancellation every `chunk`
// accounts.
fn ensure_code_present(
    tx: &impl DbTx,
    chunk: u64,
    cancel: &CancellationToken,
) -> Result<(), SnapSyncError> {
    let chunk = chunk.max(1);
    let mut cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    for (scanned, entry) in cursor.walk(None)?.enumerate() {
        if (scanned as u64).is_multiple_of(chunk) && cancel.is_cancelled() {
            return Err(SnapSyncError::Cancelled)
        }
        let (_, account) = entry?;
        // Only presence matters, so stored code is not decoded.
        if let Some(hash) = account.bytecode_hash.filter(|hash| *hash != KECCAK256_EMPTY) &&
            tx.get::<RawTable<tables::Bytecodes>>(RawKey::new(hash))?.is_none()
        {
            return Err(SnapSyncError::MissingCode { hash })
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{account, hashed_factory, header, key, state_root},
        SnapGeneration,
    };
    use alloy_consensus::TxLegacy;
    use alloy_eips::eip4895::{Withdrawal, Withdrawals};
    use alloy_primitives::{map::B256Map, Address, Bytes, Signature, U256};
    use reth_db_api::transaction::DbTxMut;
    use reth_ethereum_primitives::{BlockBody, Receipt, Transaction, TransactionSigned};
    use reth_primitives_traits::SealedHeader;
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        BlockBodyIndicesProvider, DatabaseProviderFactory, EitherWriter, ProviderFactory,
        PruneCheckpointReader, StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
        StorageSettings, StorageSettingsCache, TransactionsProvider,
    };
    use reth_prune_types::{PruneMode, PruneModes, ReceiptsLogPruneConfig};
    use reth_stages::stages::MerkleStage;
    use reth_stages_api::{ExecInput, Stage, StageError};
    use reth_trie_common::{root::storage_root_unsorted, HashedStorage, TrieAccount};
    use revm::bytecode::Bytecode;
    use std::collections::BTreeMap;

    type Factory = ProviderFactory<MockNodeTypesWithDB>;
    type Provider = <Factory as DatabaseProviderFactory>::ProviderRW;

    const CONTRACT: B256 = B256::repeat_byte(0xaa);
    const SLOT: B256 = B256::repeat_byte(0x55);

    fn code() -> Bytecode {
        Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]))
    }

    fn storage() -> HashedStorage {
        HashedStorage::from_iter([(SLOT, U256::from(7))])
    }

    // Two plain accounts and a contract with one slot and code.
    fn accounts() -> Vec<(B256, TrieAccount)> {
        let mut contract = account(3);
        contract.storage_root = storage_root_unsorted([(SLOT, U256::from(7))]);
        contract.code_hash = code().hash_slow();
        vec![(key(1), account(1)), (key(2), account(2)), (CONTRACT, contract)]
    }

    // Blocks 0 through 2, each header committing to `header_root`.
    fn insert_chain(factory: &Factory, header_root: B256) -> Vec<BlockNumHash> {
        let mut parent = B256::ZERO;
        let headers: Vec<_> = (0..=2)
            .map(|number| {
                let mut header = header(number, parent, None);
                header.state_root = header_root;
                let sealed = SealedHeader::seal_slow(header);
                parent = sealed.hash();
                sealed
            })
            .collect();
        insert_headers(factory, &headers);
        headers.iter().map(|header| BlockNumHash::new(header.number, header.hash())).collect()
    }

    // An attempt pivoted at block 1 of a chain committing to `header_root`, with `served` of the
    // accounts downloaded.
    fn downloaded(header_root: B256, served: usize) -> (Factory, SnapWrite, Vec<BlockNumHash>) {
        let accounts = accounts();
        let factory = hashed_factory();
        let blocks = insert_chain(&factory, header_root);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider
            .start_snap_attempt(SnapGeneration::new(blocks[1], state_root(&accounts)))
            .unwrap();
        provider.start_account_coverage(write).unwrap();
        let targets = (served < accounts.len()).then(|| accounts[served - 1].0);
        let range =
            crate::test_utils::verified_range(&accounts, 0..served, B256::ZERO, targets.as_slice());
        let complete = served == accounts.len();
        let (storages, bytecodes) = if complete {
            (B256Map::from_iter([(CONTRACT, storage())]), vec![(code().hash_slow(), code())])
        } else {
            Default::default()
        };
        provider.commit_account_range(write, &range, storages, bytecodes).unwrap();
        provider.commit().unwrap();
        (factory, write, blocks)
    }

    fn start(provider: &Provider, write: SnapWrite) -> Result<(), SnapSyncError> {
        provider.start_trie_rebuild(write, 1, &CancellationToken::new())
    }

    // Runs the merkle stage from its checkpoint to `target`, as the pipeline would.
    fn run_merkle(provider: &Provider, target: u64) -> Result<(), StageError> {
        let mut stage = MerkleStage::default_execution();
        loop {
            let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap();
            let output = stage.execute(provider, ExecInput { target: Some(target), checkpoint })?;
            provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint).unwrap();
            if output.done {
                return Ok(())
            }
        }
    }

    fn merkle_checkpoint(provider: &Provider) -> Option<u64> {
        provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap().map(|c| c.block_number)
    }

    #[test]
    fn complete_state_is_verified_once_the_merkle_stage_reaches_the_pivot() {
        let root = state_root(&accounts());
        let (factory, write, blocks) = downloaded(root, accounts().len());
        let provider = factory.database_provider_rw().unwrap();

        start(&provider, write).unwrap();
        run_merkle(&provider, 1).unwrap();
        let verified = provider.verify_state_root(write).unwrap();
        provider.commit().unwrap();

        assert_eq!((verified.target(), verified.state_root()), (blocks[1], root));
        let provider = factory.database_provider_rw().unwrap();
        assert!(provider.snap_attempt().unwrap().unwrap().is_verified());
        // Verified state accepts no further writes, including a second verification.
        assert!(matches!(provider.verify_state_root(write), Err(SnapSyncError::StaleWrite { .. })));
    }

    #[test]
    fn unfinished_accounts_prevent_the_hand_off() {
        let (factory, write, _) = downloaded(state_root(&accounts()), 1);
        let provider = factory.database_provider_rw().unwrap();

        assert!(matches!(
            start(&provider, write),
            Err(SnapSyncError::IncompleteAccounts { next }) if next == key(2)
        ));
        assert_eq!(merkle_checkpoint(&provider), None);
    }

    #[test]
    fn missing_code_prevents_the_hand_off() {
        let (factory, write, _) = downloaded(state_root(&accounts()), accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        provider.tx_ref().delete::<tables::Bytecodes>(code().hash_slow(), None).unwrap();

        assert!(matches!(
            start(&provider, write),
            Err(SnapSyncError::MissingCode { hash }) if hash == code().hash_slow()
        ));
    }

    #[test]
    fn catch_up_short_of_the_pivot_prevents_the_hand_off() {
        let root = state_root(&accounts());
        let (factory, write, blocks) = downloaded(root, accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        let write =
            provider.advance_snap_pivot(write, SnapGeneration::new(blocks[2], root)).unwrap();

        assert!(matches!(
            start(&provider, write),
            Err(SnapSyncError::CatchUpBehindPivot { applied: 1, pivot: 2 })
        ));
    }

    #[test]
    fn a_cancelled_session_stops_the_completeness_scan() {
        let (factory, write, _) = downloaded(state_root(&accounts()), accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        assert!(matches!(
            provider.start_trie_rebuild(write, 1, &cancel),
            Err(SnapSyncError::Cancelled)
        ));
        assert_eq!(merkle_checkpoint(&provider), None);
    }

    #[test]
    fn corrupted_state_never_reaches_the_pivot() {
        let (factory, write, _) = downloaded(state_root(&accounts()), accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        start(&provider, write).unwrap();
        provider.tx_ref().delete::<tables::HashedStorages>(CONTRACT, None).unwrap();

        // The stage refuses a root other than the header's.
        assert!(matches!(run_merkle(&provider, 1), Err(StageError::Block { .. })));
        assert!(matches!(
            provider.verify_state_root(write),
            Err(SnapSyncError::Provider(ProviderError::StateForNumberNotFound(1)))
        ));
        assert!(provider.snap_attempt().unwrap().unwrap().is_unfinished());
    }

    #[test]
    fn an_interrupted_rebuild_is_not_verified() {
        let (factory, write, _) = downloaded(state_root(&accounts()), accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        // Progress recorded before the hand-off describes other state.
        provider.save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::new(1)).unwrap();

        start(&provider, write).unwrap();

        assert!(matches!(
            provider.verify_state_root(write),
            Err(SnapSyncError::Provider(ProviderError::StateForNumberNotFound(1)))
        ));
        run_merkle(&provider, 1).unwrap();
        provider.verify_state_root(write).unwrap();
    }

    #[test]
    fn a_header_committing_to_another_root_is_refused() {
        // Ranges authenticate against the attempt's root, but completion answers to the header.
        let header_root = B256::repeat_byte(0xcc);
        let (factory, write, _) = downloaded(header_root, accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        start(&provider, write).unwrap();

        assert!(matches!(run_merkle(&provider, 1), Err(StageError::Block { .. })));
        match provider.verify_state_root(write) {
            Err(SnapSyncError::Provider(ProviderError::StateRootMismatch(mismatch))) => {
                assert_eq!(
                    mismatch.root,
                    GotExpected { got: state_root(&accounts()), expected: header_root }
                );
            }
            other => panic!("expected a state root mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_rebuild_for_an_earlier_attempt_is_not_trusted() {
        let root = state_root(&accounts());
        let (factory, write, blocks) = downloaded(root, accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        start(&provider, write).unwrap();
        run_merkle(&provider, 1).unwrap();

        // A new attempt at the same pivot, with nothing downloaded yet.
        let restarted = provider.start_snap_attempt(SnapGeneration::new(blocks[1], root)).unwrap();

        assert_eq!(merkle_checkpoint(&provider), Some(1));
        assert!(matches!(
            provider.verify_state_root(restarted),
            Err(SnapSyncError::Provider(ProviderError::StateForNumberNotFound(1)))
        ));
    }

    #[test]
    fn moving_the_pivot_after_the_hand_off_requires_a_new_one() {
        let root = state_root(&accounts());
        let (factory, write, blocks) = downloaded(root, accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        start(&provider, write).unwrap();
        let advanced =
            provider.advance_snap_pivot(write, SnapGeneration::new(blocks[2], root)).unwrap();

        run_merkle(&provider, 2).unwrap();

        assert!(matches!(
            provider.verify_state_root(advanced),
            Err(SnapSyncError::Provider(ProviderError::StateForNumberNotFound(2)))
        ));
    }

    #[test]
    fn a_genesis_pivot_is_not_handed_off() {
        let root = state_root(&accounts());
        let (factory, _, blocks) = downloaded(root, accounts().len());
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(SnapGeneration::new(blocks[0], root)).unwrap();

        assert!(matches!(start(&provider, write), Err(SnapSyncError::GenesisPivot)));
        assert_eq!(merkle_checkpoint(&provider), None);
    }

    #[test]
    fn publishing_moves_the_covered_stages_while_the_trie_and_finish_wait() {
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();

        provider.publish_snap_state(7).unwrap();

        // Every stage must be classified, so a new one fails here until it is.
        let kept = [StageId::Era, StageId::Headers, StageId::MerkleExecute, StageId::Finish];
        for stage in StageId::ALL {
            let published = PUBLISHED_STAGES.contains(&stage);
            assert_ne!(published, kept.contains(&stage), "{stage} must be published or kept");
            let expected = published.then(|| StageCheckpoint::new(7));
            assert_eq!(provider.get_stage_checkpoint(stage).unwrap(), expected, "{stage}");
        }
    }

    #[test]
    fn publishing_refuses_the_genesis_pivot() {
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();

        assert!(matches!(provider.publish_snap_state(0), Err(SnapSyncError::GenesisPivot)));
    }

    #[test]
    fn publishing_records_the_history_below_the_pivot_as_pruned() {
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();

        provider.publish_snap_state(7).unwrap();

        for segment in PruneSegment::variants() {
            let checkpoint = provider.get_prune_checkpoint(segment).unwrap();
            assert_eq!(checkpoint, Some(PruneCheckpoint::pruned_through(7)), "{segment}");
        }
    }

    #[test]
    fn bodies_downloaded_past_the_pivot_can_be_written_again_after_publishing() {
        let factory = hashed_factory();
        insert_chain(&factory, B256::ZERO);
        let body = BlockBody {
            withdrawals: Some(Withdrawals::new(vec![Withdrawal::default()])),
            ..Default::default()
        };
        let provider = factory.database_provider_rw().unwrap();
        provider
            .append_block_bodies(vec![
                (0, None),
                (1, Some(&body)),
                (2, Some(&body)),
                (3, Some(&body)),
            ])
            .unwrap();
        provider.commit().unwrap();

        // Publish at block 1 and reset the transaction file to it, as the caller does.
        let static_files = factory.static_file_provider();
        let provider = factory.database_provider_rw().unwrap();
        provider.publish_snap_state(1).unwrap();
        static_files.delete_segment(StaticFileSegment::Transactions).unwrap();
        static_files
            .latest_writer(StaticFileSegment::Transactions)
            .unwrap()
            .ensure_at_block(1)
            .unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_rw().unwrap();
        provider.append_block_bodies(vec![(2, Some(&body))]).unwrap();
        provider.commit().unwrap();
    }

    #[test]
    fn published_state_can_unwind_bodies_to_the_pivot() {
        for tx_count in [0, 2] {
            let factory = hashed_factory();
            insert_chain(&factory, B256::ZERO);
            let provider = factory.database_provider_rw().unwrap();
            provider.publish_snap_state(1).unwrap();
            factory
                .static_file_provider()
                .latest_writer(StaticFileSegment::Transactions)
                .unwrap()
                .ensure_at_block(1)
                .unwrap();
            provider.commit().unwrap();

            let body = BlockBody {
                transactions: (0..tx_count)
                    .map(|nonce| {
                        TransactionSigned::new_unhashed(
                            Transaction::Legacy(TxLegacy { nonce, ..Default::default() }),
                            Signature::test_signature(),
                        )
                    })
                    .collect(),
                ..Default::default()
            };
            let provider = factory.database_provider_rw().unwrap();
            provider.append_block_bodies(vec![(2, Some(&body))]).unwrap();
            provider.commit().unwrap();

            let provider = factory.database_provider_rw().unwrap();
            assert_eq!(provider.block_body_indices(1).unwrap(), None);
            assert_eq!(provider.next_tx_num_after_block(1).unwrap(), 0);
            assert_eq!(provider.next_tx_num_after_block(2).unwrap(), tx_count);
            provider.remove_bodies_above(1).unwrap();
            provider.commit().unwrap();

            let provider = factory.database_provider_ro().unwrap();
            assert_eq!(provider.block_body_indices(1).unwrap(), None);
            assert_eq!(provider.block_body_indices(2).unwrap(), None);
            let static_files = factory.static_file_provider();
            assert_eq!(
                static_files.get_highest_static_file_block(StaticFileSegment::Transactions),
                Some(1)
            );
            assert_eq!(
                static_files.get_highest_static_file_tx(StaticFileSegment::Transactions),
                None
            );
        }
    }

    #[test]
    fn startup_heals_an_interrupted_first_write_after_publishing() {
        // An empty block is covered too, since transaction counts alone can't see its height.
        for tx_count in [0, 1] {
            let factory = hashed_factory();
            insert_chain(&factory, B256::ZERO);
            let static_files = factory.static_file_provider();
            let provider = factory.database_provider_rw().unwrap();
            provider.publish_snap_state(1).unwrap();
            static_files
                .latest_writer(StaticFileSegment::Transactions)
                .unwrap()
                .ensure_at_block(1)
                .unwrap();
            provider.commit().unwrap();

            let body = BlockBody {
                transactions: (0..tx_count)
                    .map(|nonce| {
                        TransactionSigned::new_unhashed(
                            Transaction::Legacy(TxLegacy { nonce, ..Default::default() }),
                            Signature::test_signature(),
                        )
                    })
                    .collect(),
                ..Default::default()
            };

            // The static files commit, the database does not.
            let provider = factory.database_provider_rw().unwrap();
            provider.append_block_bodies(vec![(2, Some(&body))]).unwrap();
            static_files.commit().unwrap();
            drop(provider);

            let provider = factory.provider().unwrap();
            assert_eq!(static_files.check_consistency(&provider).unwrap(), None, "{tx_count}");
            assert_eq!(
                static_files.get_highest_static_file_block(StaticFileSegment::Transactions),
                Some(1),
                "{tx_count}"
            );
            drop(provider);

            let provider = factory.database_provider_rw().unwrap();
            provider.append_block_bodies(vec![(2, Some(&body))]).unwrap();
            provider.commit().unwrap();
        }
    }

    #[test]
    fn publishing_clears_transaction_lookups_in_the_active_backend() {
        for settings in [StorageSettings::v1(), StorageSettings::v2()] {
            let factory = hashed_factory();
            factory.set_storage_settings_cache(settings);
            let old_hash = B256::repeat_byte(0xaa);
            let new_hash = B256::repeat_byte(0xbb);
            let insert_lookup = |hash| {
                let provider = factory.database_provider_rw().unwrap();
                provider
                    .with_rocksdb_batch(|batch| {
                        let mut writer =
                            EitherWriter::new_transaction_hash_numbers(&provider, batch)?;
                        writer.put_transaction_hash_numbers_batch(vec![(hash, 0)], false)?;
                        Ok(((), writer.into_raw_rocksdb_batch()))
                    })
                    .unwrap();
                provider.commit().unwrap();
            };
            insert_lookup(old_hash);
            assert_eq!(
                factory.database_provider_ro().unwrap().transaction_id(old_hash).unwrap(),
                Some(0)
            );

            let provider = factory.database_provider_rw().unwrap();
            provider.publish_snap_state(7).unwrap();
            provider.commit().unwrap();
            assert_eq!(
                factory.database_provider_ro().unwrap().transaction_id(old_hash).unwrap(),
                None
            );

            insert_lookup(new_hash);
            let provider = factory.database_provider_ro().unwrap();
            assert_eq!(provider.transaction_id(old_hash).unwrap(), None);
            assert_eq!(provider.transaction_id(new_hash).unwrap(), Some(0));
        }
    }

    #[test]
    fn publishing_resets_database_receipts_before_reusing_transaction_numbers() {
        let factory = hashed_factory().with_prune_modes(PruneModes {
            receipts_log_filter: ReceiptsLogPruneConfig(BTreeMap::from([(
                Address::ZERO,
                PruneMode::Before(0),
            )])),
            ..Default::default()
        });
        insert_chain(&factory, B256::ZERO);
        let body = BlockBody {
            transactions: (0..2)
                .map(|nonce| {
                    TransactionSigned::new_unhashed(
                        Transaction::Legacy(TxLegacy { nonce, ..Default::default() }),
                        Signature::test_signature(),
                    )
                })
                .collect(),
            ..Default::default()
        };
        let provider = factory.database_provider_rw().unwrap();
        provider.append_block_bodies(vec![(0, None), (1, Some(&body))]).unwrap();
        {
            let mut writer = EitherWriter::new_receipts(&provider, 1).unwrap();
            assert!(matches!(&writer, EitherWriter::Database(_)));
            writer.append_receipt(0, &Receipt::default()).unwrap();
            writer.append_receipt(1, &Receipt::default()).unwrap();
        }
        provider.commit().unwrap();
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(1).unwrap();
        provider.publish_snap_state(1).unwrap();
        provider.commit().unwrap();
        let provider = factory.database_provider_rw().unwrap();
        provider.append_block_bodies(vec![(2, Some(&body))]).unwrap();
        {
            let mut writer = EitherWriter::new_receipts(&provider, 2).unwrap();
            writer.append_receipt(0, &Receipt::default()).unwrap();
            writer.append_receipt(1, &Receipt::default()).unwrap();
        }
        provider.commit().unwrap();
    }
}
