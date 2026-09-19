//! Rebuilds the downloaded state's trie with the existing Merkle stage.
//!
//! Stage and Snap checkpoints remain durable independently, while the generation marker is only
//! cleared after the canonical header root and completed Merkle checkpoint agree.

use crate::{
    verify::ensure_trie_rebuild, SnapAttemptStore, SnapDownloadProgress, SnapPhase, SnapStateStore,
    SnapSyncError,
};
use reth_db_api::transaction::DbTxMut;
use reth_provider::{DatabaseProviderFactory, StaticFileProviderFactory};
use reth_stages::{stages::MerkleStage, ExecInput, Stage};
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockHashReader, ChangeSetReader, DBProvider, HeaderProvider, MetadataProvider, MetadataWriter,
    PruneCheckpointWriter, StageCheckpointReader, StageCheckpointWriter, StatsReader,
    StorageChangeSetReader, StorageSettingsCache, TrieWriter,
};
use tokio_util::sync::CancellationToken;

/// Drives a clean, resumable Merkle rebuild for one downloaded generation.
#[derive(Debug)]
pub struct TrieGenerator<'a, F> {
    // Stage transactions and the Snap marker share one provider factory.
    factory: &'a F,
    // Completion clears the marker only after the final stage commit is visible.
    store: SnapStateStore<'a, F>,
}

impl<'a, F> TrieGenerator<'a, F> {
    /// Creates a trie generator without opening a database transaction.
    pub const fn new(factory: &'a F) -> Self {
        Self { factory, store: SnapStateStore::new(factory) }
    }

    /// Resumes the Merkle stage and accepts the generation after root validation.
    pub fn run(&self, generation: SnapDownloadProgress) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW: DBProvider<Tx: DbTxMut>
            + MetadataProvider
            + MetadataWriter
            + BlockHashReader
            + ChangeSetReader
            + HeaderProvider
            + PruneCheckpointWriter
            + StageCheckpointReader
            + StageCheckpointWriter
            + StatsReader
            + StorageChangeSetReader
            + StorageSettingsCache
            + StaticFileProviderFactory
            + TrieWriter,
    {
        if generation.phase != SnapPhase::Trie {
            return Err(SnapSyncError::UnexpectedPhase {
                expected: SnapPhase::Trie,
                actual: generation.phase,
            });
        }
        {
            let provider = self.factory.database_provider_rw().map_err(crate::error::db_error)?;
            self.store.ensure_generation(&provider, generation)?;
            let write = provider.active_snap_write()?.ok_or(SnapSyncError::NoAttempt)?;
            ensure_trie_rebuild(&provider, write, &CancellationToken::new())?;
            provider.commit().map_err(crate::error::db_error)?;
        }
        let mut stage = MerkleStage::default_execution();
        loop {
            let provider = self.factory.database_provider_rw().map_err(crate::error::db_error)?;
            self.store.ensure_generation(&provider, generation)?;
            let checkpoint = provider
                .get_stage_checkpoint(StageId::MerkleExecute)
                .map_err(crate::error::db_error)?;
            let output = stage
                .execute(&provider, ExecInput { target: Some(generation.target_block), checkpoint })
                .map_err(|error| SnapSyncError::Trie(error.to_string()))?;
            provider
                .save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint)
                .map_err(crate::error::db_error)?;
            provider.commit().map_err(crate::error::db_error)?;
            if output.done {
                break;
            }
        }
        self.store.finish_generation(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloy_consensus::Header;
    use alloy_primitives::{B256, U256};
    use reth_db_api::{tables, transaction::DbTx};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, ProviderFactory, StaticFileProviderFactory,
        StaticFileWriter,
    };
    use reth_stages_types::StageCheckpoint;
    use reth_static_file_types::StaticFileSegment;
    use reth_trie_common::{HashBuilder, HashedPostState, Nibbles, TrieAccount, EMPTY_ROOT_HASH};

    fn downloaded(
    ) -> (ProviderFactory<MockNodeTypesWithDB>, SnapDownloadProgress, B256, TrieAccount) {
        let factory = crate::test_utils::hashed_factory();
        let account_hash = B256::repeat_byte(0x11);
        let trie_account = TrieAccount {
            nonce: 3,
            balance: U256::from(4),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: alloy_primitives::KECCAK256_EMPTY,
        };
        let mut builder = HashBuilder::default();
        builder.add_leaf(Nibbles::unpack(account_hash), &alloy_rlp::encode(trie_account));
        let state_root = builder.root();
        let header0 = Header::default();
        let hash0 = alloy_primitives::Sealable::hash_slow(&header0);
        let header1 = Header { number: 1, parent_hash: hash0, state_root, ..Default::default() };
        let hash1 = alloy_primitives::Sealable::hash_slow(&header1);
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.append_header(&header0, &hash0).unwrap();
        writer.append_header(&header1, &hash1).unwrap();
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);

        let store = SnapStateStore::new(&factory);
        let generation = SnapDownloadProgress::new(1, hash1, state_root);
        store.begin_generation(generation).unwrap();
        let generation = store
            .seed_account_state(
                generation,
                HashedPostState::default()
                    .with_accounts([(account_hash, Some(Account::from(trie_account)))]),
                Vec::new(),
                None,
            )
            .unwrap();
        let generation = store.complete_block_access_lists(generation).unwrap();
        (factory, generation, account_hash, trie_account)
    }

    #[test]
    fn rebuilds_with_merkle_stage_before_clearing_marker() {
        let (factory, generation, account_hash, trie_account) = downloaded();
        let store = SnapStateStore::new(&factory);

        TrieGenerator::new(&factory).run(generation).unwrap();

        assert_eq!(store.interrupted_generation().unwrap(), None);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap().unwrap().block_number,
            1
        );
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(account_hash).unwrap(),
            Some(Account::from(trie_account))
        );
        assert!(provider.snap_attempt().unwrap().unwrap().is_verified());
        assert_eq!(store.completed_block().unwrap(), Some(1));
    }

    #[test]
    fn missing_bytecode_prevents_rebuild_and_publication() {
        let (factory, generation, account_hash, trie_account) = downloaded();
        let code_hash = B256::repeat_byte(0xcc);
        let provider = factory.database_provider_rw().unwrap();
        let account = Account { bytecode_hash: Some(code_hash), ..Account::from(trie_account) };
        provider.tx_ref().put::<tables::HashedAccounts>(account_hash, account).unwrap();
        provider.commit().unwrap();

        assert!(matches!(
            TrieGenerator::new(&factory).run(generation),
            Err(SnapSyncError::MissingCode { hash }) if hash == code_hash
        ));
        let store = SnapStateStore::new(&factory);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
        assert_eq!(store.completed_block().unwrap(), None);
    }

    #[test]
    fn a_legacy_checkpoint_cannot_bypass_root_validation() {
        let (factory, generation, account_hash, trie_account) = downloaded();
        let provider = factory.database_provider_rw().unwrap();
        let account = Account { balance: U256::from(99), ..Account::from(trie_account) };
        provider.tx_ref().put::<tables::HashedAccounts>(account_hash, account).unwrap();
        // A completed checkpoint without this attempt's hand-off must not authenticate state.
        provider.save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::new(1)).unwrap();
        provider.commit().unwrap();

        assert!(matches!(
            TrieGenerator::new(&factory).run(generation),
            Err(SnapSyncError::Trie(_))
        ));
        let store = SnapStateStore::new(&factory);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
        assert_eq!(store.completed_block().unwrap(), None);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.snap_attempt().unwrap().unwrap().is_unfinished());
    }
}
