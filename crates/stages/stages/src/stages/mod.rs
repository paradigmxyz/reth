/// The bodies stage.
mod bodies;
/// The execution stage that generates state diff.
mod execution;
/// The finish stage
mod finish;
/// Account hashing stage.
mod hashing_account;
/// Storage hashing stage.
mod hashing_storage;
/// The headers stage.
mod headers;
/// Index history of account changes
mod index_account_history;
/// Index history of storage changes
mod index_storage_history;
/// Stage for computing state root.
mod merkle;
mod prune;
/// The sender recovery stage.
mod sender_recovery;
/// The transaction lookup stage
mod tx_lookup;

pub use bodies::*;
pub use execution::*;
pub use finish::*;
pub use hashing_account::*;
pub use hashing_storage::*;
pub use headers::*;
pub use index_account_history::*;
pub use index_storage_history::*;
pub use merkle::*;
pub use prune::*;
pub use sender_recovery::*;
pub use tx_lookup::*;

mod utils;
use utils::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{StorageKind, TestStageDB};
    use alloy_primitives::{address, hex_literal::hex, keccak256, BlockNumber, B256, U256};
    use alloy_rlp::Decodable;
    use reth_chainspec::ChainSpecBuilder;
    use reth_db::{
        mdbx::{cursor::Cursor, RW},
        tables, AccountsHistory,
    };
    use reth_db_api::{
        cursor::{DbCursorRO, DbCursorRW},
        table::Table,
        transaction::{DbTx, DbTxMut},
    };
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_exex::ExExManagerHandle;
    use reth_primitives::{Account, Bytecode, SealedBlock, StaticFileSegment};
    use reth_provider::{
        providers::{StaticFileProvider, StaticFileWriter},
        test_utils::MockNodeTypesWithDB,
        AccountExtReader, BlockReader, DatabaseProviderFactory, ProviderFactory, ProviderResult,
        ReceiptProvider, StageCheckpointWriter, StaticFileProviderFactory, StorageReader,
    };
    use reth_prune_types::{PruneMode, PruneModes};
    use reth_stages_api::{
        ExecInput, ExecutionStageThresholds, PipelineTarget, Stage, StageCheckpoint, StageId,
    };
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_receipt, BlockRangeParams,
    };
    use std::{io::Write, sync::Arc};

    #[tokio::test]
    #[ignore]
    async fn test_prune() {
        let test_db = TestStageDB::default();

        let provider_rw = test_db.factory.provider_rw().unwrap();
        let tip = 66;
        let input = ExecInput { target: Some(tip), checkpoint: None };
        let mut genesis_rlp = hex!("f901faf901f5a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa045571b40ae66ca7480791bbb2887286e4e4c4b1b298b191c889d6959023a32eda056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808502540be400808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        provider_rw.insert_historical_block(genesis.try_seal_with_senders().unwrap()).unwrap();
        provider_rw
            .insert_historical_block(block.clone().try_seal_with_senders().unwrap())
            .unwrap();

        // Fill with bogus blocks to respect PruneMode distance.
        let mut head = block.hash();
        let mut rng = generators::rng();
        for block_number in 2..=tip {
            let nblock = random_block(
                &mut rng,
                block_number,
                generators::BlockParams { parent: Some(head), ..Default::default() },
            );
            head = nblock.hash();
            provider_rw.insert_historical_block(nblock.try_seal_with_senders().unwrap()).unwrap();
        }
        provider_rw
            .static_file_provider()
            .latest_writer(StaticFileSegment::Headers)
            .unwrap()
            .commit()
            .unwrap();
        provider_rw.commit().unwrap();

        // insert pre state
        let provider_rw = test_db.factory.provider_rw().unwrap();
        let code = hex!("5a465a905090036002900360015500");
        let code_hash = keccak256(hex!("5a465a905090036002900360015500"));
        provider_rw
            .tx_ref()
            .put::<tables::PlainAccountState>(
                address!("1000000000000000000000000000000000000000"),
                Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) },
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::PlainAccountState>(
                address!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"),
                Account {
                    nonce: 0,
                    balance: U256::from(0x3635c9adc5dea00000u128),
                    bytecode_hash: None,
                },
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into()))
            .unwrap();
        provider_rw.commit().unwrap();

        let check_pruning = |factory: ProviderFactory<MockNodeTypesWithDB>,
                             prune_modes: PruneModes,
                             expect_num_receipts: usize,
                             expect_num_acc_changesets: usize,
                             expect_num_storage_changesets: usize| async move {
            let provider = factory.database_provider_rw().unwrap();

            // Check execution and create receipts and changesets according to the pruning
            // configuration
            let mut execution_stage = ExecutionStage::new(
                EthExecutorProvider::ethereum(Arc::new(
                    ChainSpecBuilder::mainnet().berlin_activated().build(),
                )),
                ExecutionStageThresholds {
                    max_blocks: Some(100),
                    max_changes: None,
                    max_cumulative_gas: None,
                    max_duration: None,
                },
                MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
                prune_modes.clone(),
                ExExManagerHandle::empty(),
            );

            execution_stage.execute(&provider, input).unwrap();
            assert_eq!(
                provider.receipts_by_block(1.into()).unwrap().unwrap().len(),
                expect_num_receipts
            );

            assert_eq!(
                provider.changed_storages_and_blocks_with_range(0..=1000).unwrap().len(),
                expect_num_storage_changesets
            );

            assert_eq!(
                provider.changed_accounts_and_blocks_with_range(0..=1000).unwrap().len(),
                expect_num_acc_changesets
            );

            // Check AccountHistory
            let mut acc_indexing_stage = IndexAccountHistoryStage {
                prune_mode: prune_modes.account_history,
                ..Default::default()
            };

            if prune_modes.account_history == Some(PruneMode::Full) {
                // Full is not supported
                assert!(acc_indexing_stage.execute(&provider, input).is_err());
            } else {
                acc_indexing_stage.execute(&provider, input).unwrap();
                let mut account_history: Cursor<RW, AccountsHistory> =
                    provider.tx_ref().cursor_read::<tables::AccountsHistory>().unwrap();
                assert_eq!(account_history.walk(None).unwrap().count(), expect_num_acc_changesets);
            }

            // Check StorageHistory
            let mut storage_indexing_stage = IndexStorageHistoryStage {
                prune_mode: prune_modes.storage_history,
                ..Default::default()
            };

            if prune_modes.storage_history == Some(PruneMode::Full) {
                // Full is not supported
                assert!(acc_indexing_stage.execute(&provider, input).is_err());
            } else {
                storage_indexing_stage.execute(&provider, input).unwrap();

                let mut storage_history =
                    provider.tx_ref().cursor_read::<tables::StoragesHistory>().unwrap();
                assert_eq!(
                    storage_history.walk(None).unwrap().count(),
                    expect_num_storage_changesets
                );
            }
        };

        // In an unpruned configuration there is 1 receipt, 3 changed accounts and 1 changed
        // storage.
        let mut prune = PruneModes::none();
        check_pruning(test_db.factory.clone(), prune.clone(), 1, 3, 1).await;

        prune.receipts = Some(PruneMode::Full);
        prune.account_history = Some(PruneMode::Full);
        prune.storage_history = Some(PruneMode::Full);
        // This will result in error for account_history and storage_history, which is caught.
        check_pruning(test_db.factory.clone(), prune.clone(), 0, 0, 0).await;

        prune.receipts = Some(PruneMode::Before(1));
        prune.account_history = Some(PruneMode::Before(1));
        prune.storage_history = Some(PruneMode::Before(1));
        check_pruning(test_db.factory.clone(), prune.clone(), 1, 3, 1).await;

        prune.receipts = Some(PruneMode::Before(2));
        prune.account_history = Some(PruneMode::Before(2));
        prune.storage_history = Some(PruneMode::Before(2));
        // The one account is the miner
        check_pruning(test_db.factory.clone(), prune.clone(), 0, 1, 0).await;

        prune.receipts = Some(PruneMode::Distance(66));
        prune.account_history = Some(PruneMode::Distance(66));
        prune.storage_history = Some(PruneMode::Distance(66));
        check_pruning(test_db.factory.clone(), prune.clone(), 1, 3, 1).await;

        prune.receipts = Some(PruneMode::Distance(64));
        prune.account_history = Some(PruneMode::Distance(64));
        prune.storage_history = Some(PruneMode::Distance(64));
        // The one account is the miner
        check_pruning(test_db.factory.clone(), prune.clone(), 0, 1, 0).await;
    }

    /// It will generate `num_blocks`, push them to static files and set all stage checkpoints to
    /// `num_blocks - 1`.
    fn seed_data(num_blocks: usize) -> ProviderResult<TestStageDB> {
        let db = TestStageDB::default();
        let mut rng = generators::rng();
        let genesis_hash = B256::ZERO;
        let tip = (num_blocks - 1) as u64;

        let blocks = random_block_range(
            &mut rng,
            0..=tip,
            BlockRangeParams { parent: Some(genesis_hash), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Static)?;

        let mut receipts = Vec::new();
        let mut tx_num = 0u64;
        for block in &blocks {
            let mut block_receipts = Vec::with_capacity(block.body.transactions.len());
            for transaction in &block.body.transactions {
                block_receipts.push((tx_num, random_receipt(&mut rng, transaction, Some(0))));
                tx_num += 1;
            }
            receipts.push((block.number, block_receipts));
        }
        db.insert_receipts_by_block(receipts, StorageKind::Static)?;

        // simulate pipeline by setting all checkpoints to inserted height.
        let provider_rw = db.factory.provider_rw()?;
        for stage in StageId::ALL {
            provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(tip))?;
        }
        provider_rw.commit()?;

        Ok(db)
    }

    /// Simulates losing data to corruption and compare the check consistency result
    /// against the expected one.
    fn simulate_behind_checkpoint_corruption(
        db: &TestStageDB,
        prune_count: usize,
        segment: StaticFileSegment,
        is_full_node: bool,
        expected: Option<PipelineTarget>,
    ) {
        // We recreate the static file provider, since consistency heals are done on fetching the
        // writer for the first time.
        let static_file_provider =
            StaticFileProvider::read_write(db.factory.static_file_provider().path()).unwrap();

        // Simulate corruption by removing `prune_count` rows from the data file without updating
        // its offset list and configuration.
        {
            let mut headers_writer = static_file_provider.latest_writer(segment).unwrap();
            let reader = headers_writer.inner().jar().open_data_reader().unwrap();
            let columns = headers_writer.inner().jar().columns();
            let data_file = headers_writer.inner().data_file();
            let last_offset = reader.reverse_offset(prune_count * columns).unwrap();
            data_file.get_mut().set_len(last_offset).unwrap();
            data_file.flush().unwrap();
            data_file.get_ref().sync_all().unwrap();
        }

        // We recreate the static file provider, since consistency heals are done on fetching the
        // writer for the first time.
        assert_eq!(
            StaticFileProvider::read_write(db.factory.static_file_provider().path())
                .unwrap()
                .check_consistency(&db.factory.database_provider_ro().unwrap(), is_full_node,),
            Ok(expected)
        );
    }

    /// Saves a checkpoint with `checkpoint_block_number` and compare the check consistency result
    /// against the expected one.
    fn save_checkpoint_and_check(
        db: &TestStageDB,
        stage_id: StageId,
        checkpoint_block_number: BlockNumber,
        expected: Option<PipelineTarget>,
    ) {
        let provider_rw = db.factory.provider_rw().unwrap();
        provider_rw
            .save_stage_checkpoint(stage_id, StageCheckpoint::new(checkpoint_block_number))
            .unwrap();
        provider_rw.commit().unwrap();

        assert_eq!(
            db.factory
                .static_file_provider()
                .check_consistency(&db.factory.database_provider_ro().unwrap(), false,),
            Ok(expected)
        );
    }

    /// Inserts a dummy value at key and compare the check consistency result against the expected
    /// one.
    fn update_db_and_check<T: Table<Key = u64>>(
        db: &TestStageDB,
        key: u64,
        expected: Option<PipelineTarget>,
    ) where
        <T as Table>::Value: Default,
    {
        let provider_rw = db.factory.provider_rw().unwrap();
        let mut cursor = provider_rw.tx_ref().cursor_write::<T>().unwrap();
        cursor.insert(key, Default::default()).unwrap();
        provider_rw.commit().unwrap();

        assert_eq!(
            db.factory
                .static_file_provider()
                .check_consistency(&db.factory.database_provider_ro().unwrap(), false),
            Ok(expected)
        );
    }

    #[test]
    fn test_consistency() {
        let db = seed_data(90).unwrap();
        let db_provider = db.factory.database_provider_ro().unwrap();

        assert_eq!(
            db.factory.static_file_provider().check_consistency(&db_provider, false),
            Ok(None)
        );
    }

    #[test]
    fn test_consistency_no_commit_prune() {
        let db = seed_data(90).unwrap();
        let full_node = true;
        let archive_node = !full_node;

        // Full node does not use receipts, therefore doesn't check for consistency on receipts
        // segment
        simulate_behind_checkpoint_corruption(&db, 1, StaticFileSegment::Receipts, full_node, None);

        // there are 2 to 3 transactions per block. however, if we lose one tx, we need to unwind to
        // the previous block.
        simulate_behind_checkpoint_corruption(
            &db,
            1,
            StaticFileSegment::Receipts,
            archive_node,
            Some(PipelineTarget::Unwind(88)),
        );

        simulate_behind_checkpoint_corruption(
            &db,
            3,
            StaticFileSegment::Headers,
            archive_node,
            Some(PipelineTarget::Unwind(86)),
        );
    }

    #[test]
    fn test_consistency_checkpoints() {
        let db = seed_data(90).unwrap();

        // When a checkpoint is behind, we delete data from static files.
        let block = 87;
        save_checkpoint_and_check(&db, StageId::Bodies, block, None);
        assert_eq!(
            db.factory
                .static_file_provider()
                .get_highest_static_file_block(StaticFileSegment::Transactions),
            Some(block)
        );
        assert_eq!(
            db.factory
                .static_file_provider()
                .get_highest_static_file_tx(StaticFileSegment::Transactions),
            db.factory.block_body_indices(block).unwrap().map(|b| b.last_tx_num())
        );

        let block = 86;
        save_checkpoint_and_check(&db, StageId::Execution, block, None);
        assert_eq!(
            db.factory
                .static_file_provider()
                .get_highest_static_file_block(StaticFileSegment::Receipts),
            Some(block)
        );
        assert_eq!(
            db.factory
                .static_file_provider()
                .get_highest_static_file_tx(StaticFileSegment::Receipts),
            db.factory.block_body_indices(block).unwrap().map(|b| b.last_tx_num())
        );

        let block = 80;
        save_checkpoint_and_check(&db, StageId::Headers, block, None);
        assert_eq!(
            db.factory
                .static_file_provider()
                .get_highest_static_file_block(StaticFileSegment::Headers),
            Some(block)
        );

        // When a checkpoint is ahead, we request a pipeline unwind.
        save_checkpoint_and_check(&db, StageId::Headers, 91, Some(PipelineTarget::Unwind(block)));
    }

    #[test]
    fn test_consistency_headers_gap() {
        let db = seed_data(90).unwrap();
        let current = db
            .factory
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .unwrap();

        // Creates a gap of one header: static_file <missing> db
        update_db_and_check::<tables::Headers>(&db, current + 2, Some(PipelineTarget::Unwind(89)));

        // Fill the gap, and ensure no unwind is necessary.
        update_db_and_check::<tables::Headers>(&db, current + 1, None);
    }

    #[test]
    fn test_consistency_tx_gap() {
        let db = seed_data(90).unwrap();
        let current = db
            .factory
            .static_file_provider()
            .get_highest_static_file_tx(StaticFileSegment::Transactions)
            .unwrap();

        // Creates a gap of one transaction: static_file <missing> db
        update_db_and_check::<tables::Transactions>(
            &db,
            current + 2,
            Some(PipelineTarget::Unwind(89)),
        );

        // Fill the gap, and ensure no unwind is necessary.
        update_db_and_check::<tables::Transactions>(&db, current + 1, None);
    }

    #[test]
    fn test_consistency_receipt_gap() {
        let db = seed_data(90).unwrap();
        let current = db
            .factory
            .static_file_provider()
            .get_highest_static_file_tx(StaticFileSegment::Receipts)
            .unwrap();

        // Creates a gap of one receipt: static_file <missing> db
        update_db_and_check::<tables::Receipts>(&db, current + 2, Some(PipelineTarget::Unwind(89)));

        // Fill the gap, and ensure no unwind is necessary.
        update_db_and_check::<tables::Receipts>(&db, current + 1, None);
    }
}
