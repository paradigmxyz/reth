//! Decides when downloaded state is complete, and accepts it once its trie matches the pivot.
//!
//! The trie itself is rebuilt by the merkle stage, which commits its progress in chunks and checks
//! the root against the target header, so a rebuild survives restarts on any state size.

use crate::{SnapAccountStore, SnapAttemptStore, SnapCatchUpStore, SnapSyncError, SnapWrite};
use alloy_eips::BlockNumHash;
use alloy_primitives::{B256, KECCAK256_EMPTY};
use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx, RawKey, RawTable};
use reth_primitives_traits::{AlloyBlockHeader, GotExpected};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockHashReader, DBProvider, HeaderProvider, MetadataProvider, MetadataWriter, SnapAttemptId,
    StageCheckpointReader, StageCheckpointWriter,
};
use reth_storage_errors::provider::{ProviderError, RootMismatch};
use tokio_util::sync::CancellationToken;

/// Accounts scanned between cancellation checks, bounding how long a cancelled session keeps
/// running.
pub const DEFAULT_SCAN_CHUNK: u64 = 100_000;

/// Decides whether downloaded state can be trusted as the node's state.
///
/// Writes join the caller's transaction, so a refused check changes nothing.
pub trait SnapStateVerifier {
    /// Hands complete state to the merkle stage, which rebuilds its trie from scratch.
    ///
    /// Progress the stage recorded before the hand-off describes other state, so it is discarded.
    fn start_trie_rebuild(
        &self,
        write: SnapWrite,
        chunk: u64,
        cancel: &CancellationToken,
    ) -> Result<(), SnapSyncError>
    where
        Self: StageCheckpointWriter + DBProvider;

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
        Self: StageCheckpointWriter + DBProvider,
    {
        self.verify_completeness(write, chunk, cancel)?;
        // Starting from block zero makes the stage clear the trie tables and rebuild them, since
        // downloaded state has no changesets to update an existing trie from.
        self.save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::default())?;
        self.save_stage_checkpoint_progress(StageId::MerkleExecute, Vec::new())?;
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
        missing_code(self.tx_ref(), chunk, cancel)
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
        // Until the stage reaches the pivot, the trie holds no state for it.
        let rebuilt = self.get_stage_checkpoint(StageId::MerkleExecute)?;
        if rebuilt.map(|checkpoint| checkpoint.block_number) != Some(target.number) {
            return Err(ProviderError::StateForNumberNotFound(target.number).into())
        }
        self.verify_snap_attempt(write)?;
        Ok(VerifiedSnapState { attempt: attempt.id(), target, state_root: header.state_root() })
    }
}

// Refuses the first account whose code is not stored, checking for cancellation every `chunk`
// accounts.
fn missing_code(
    tx: &impl DbTx,
    chunk: u64,
    cancel: &CancellationToken,
) -> Result<(), SnapSyncError> {
    let mut cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    for (scanned, entry) in cursor.walk(None)?.enumerate() {
        if (scanned as u64).is_multiple_of(chunk.max(1)) && cancel.is_cancelled() {
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
        SnapGeneration, SnapStorageStore, StorageChunk,
    };
    use alloy_primitives::{map::B256Map, Bytes, U256};
    use reth_db_api::transaction::DbTxMut;
    use reth_primitives_traits::SealedHeader;
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        DatabaseProviderFactory, ProviderFactory,
    };
    use reth_stages::stages::MerkleStage;
    use reth_stages_api::{ExecInput, Stage, StageError};
    use reth_trie_common::{root::storage_root_unsorted, HashedStorage, TrieAccount};
    use revm::bytecode::Bytecode;

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
    fn storage_persisted_ahead_of_its_range_prevents_the_hand_off() {
        let (factory, write, _) = downloaded(state_root(&accounts()), 2);
        let provider = factory.database_provider_rw().unwrap();
        let origin = provider.account_coverage(write).unwrap().unwrap().next().unwrap();
        let root = accounts()[2].1.storage_root;
        let chunk =
            StorageChunk::new(CONTRACT, root, B256::ZERO, vec![(SLOT, U256::from(7))], None);
        provider.commit_storage_chunk(write, origin, chunk).unwrap();

        assert!(matches!(start(&provider, write), Err(SnapSyncError::IncompleteAccounts { .. })));
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
}
