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

pub use sender_recovery::*;
pub use tx_lookup::*;

mod utils;
use utils::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{stage::Stage, test_utils::TestStageDB, ExecInput};
    use alloy_rlp::Decodable;
    use reth_db::{
        cursor::DbCursorRO,
        mdbx::{cursor::Cursor, RW},
        tables,
        test_utils::TempDatabase,
        transaction::{DbTx, DbTxMut},
        AccountsHistory, DatabaseEnv,
    };
    use reth_interfaces::test_utils::generators::{self, random_block};
    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::{
        address, hex_literal::hex, keccak256, Account, Bytecode, ChainSpecBuilder, PruneMode,
        PruneModes, SealedBlock, StaticFileSegment, U256,
    };
    use reth_provider::{
        providers::StaticFileWriter, AccountExtReader, ProviderFactory, ReceiptProvider,
        StorageReader,
    };
    use reth_revm::EvmProcessorFactory;
    use std::sync::Arc;

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
        provider_rw
            .insert_historical_block(genesis.try_seal_with_senders().unwrap(), None)
            .unwrap();
        provider_rw
            .insert_historical_block(block.clone().try_seal_with_senders().unwrap(), None)
            .unwrap();

        // Fill with bogus blocks to respect PruneMode distance.
        let mut head = block.hash();
        let mut rng = generators::rng();
        for block_number in 2..=tip {
            let nblock = random_block(&mut rng, block_number, Some(head), Some(0), Some(0));
            head = nblock.hash();
            provider_rw
                .insert_historical_block(nblock.try_seal_with_senders().unwrap(), None)
                .unwrap();
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

        let check_pruning = |factory: ProviderFactory<Arc<TempDatabase<DatabaseEnv>>>,
                             prune_modes: PruneModes,
                             expect_num_receipts: usize,
                             expect_num_acc_changesets: usize,
                             expect_num_storage_changesets: usize| async move {
            let provider = factory.provider_rw().unwrap();

            // Check execution and create receipts and changesets according to the pruning
            // configuration
            let mut execution_stage = ExecutionStage::new(
                EvmProcessorFactory::new(
                    Arc::new(ChainSpecBuilder::mainnet().berlin_activated().build()),
                    EthEvmConfig::default(),
                ),
                ExecutionStageThresholds {
                    max_blocks: Some(100),
                    max_changes: None,
                    max_cumulative_gas: None,
                    max_duration: None,
                },
                MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
                prune_modes.clone(),
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
}
