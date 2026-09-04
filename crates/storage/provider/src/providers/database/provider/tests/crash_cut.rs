//! Abrupt process exits inside the production forward commit, followed by production recovery.
//! These cuts preserve the host page cache and use real MDBX, `RocksDB`, and static files. They
//! complement simulated file faults; they do not emulate power loss or cuts inside backend FFI.

use super::*;
use crate::{providers::RocksDBBuilder, test_utils::MockNodeTypesWithDB, ProviderFactory};
use alloy_consensus::{TxLegacy, TxType};
use alloy_primitives::{Signature, TxKind};
use reth_chainspec::MAINNET;
use reth_db::{mdbx::DatabaseArguments, DatabaseEnv};
use reth_ethereum_primitives::{
    calculate_receipt_root_no_memo, Block, BlockBody, Transaction, TransactionSigned,
};
use reth_primitives_traits::proofs::calculate_transaction_root;
use std::{cell::Cell, path::Path, process::Command};

const EXIT_AT_CUT: i32 = 86;
const CHILD_CUT: &str = "RETH_STORAGE_CRASH_CUT";
const CHILD_DIR: &str = "RETH_STORAGE_CRASH_DIR";

type Factory = ProviderFactory<MockNodeTypesWithDB<Arc<DatabaseEnv>>>;

thread_local! {
    // Only the committing child-test thread is armed, after the acknowledged baseline commits.
    static CUT: Cell<Option<(&'static str, usize)>> = const { Cell::new(None) };
}

pub(crate) fn after_commit_step(step: &'static str) {
    CUT.with(|cut| {
        let Some((target, remaining)) = cut.get() else { return };
        if target != step {
            return;
        }
        if remaining == 0 {
            // Skip provider/runtime Rust destructors. The parent retains ownership of the datadir.
            std::process::exit(EXIT_AT_CUT);
        }
        cut.set(Some((target, remaining - 1)));
    });
}

fn open_factory(path: &Path) -> Factory {
    let static_files = path.join("static_files");
    reth_fs_util::create_dir_all(&static_files).unwrap();
    ProviderFactory::new(
        Arc::new(reth_db::init_db(path.join("db"), DatabaseArguments::test()).unwrap()),
        MAINNET.clone(),
        StaticFileProvider::read_write(static_files).unwrap(),
        RocksDBBuilder::new(path.join("rocksdb")).with_default_tables().build().unwrap(),
        reth_tasks::Runtime::test(),
    )
    .unwrap()
}

fn account(number: u64) -> Address {
    Address::with_last_byte(number as u8)
}

fn blocks() -> Vec<ExecutedBlock> {
    let mut parent_hash = B256::ZERO;
    (0..=3)
        .map(|number| {
            let transactions = if number == 0 {
                Vec::new()
            } else {
                vec![TransactionSigned::new_unhashed(
                    Transaction::Legacy(TxLegacy {
                        nonce: number,
                        gas_limit: 21_000,
                        gas_price: 1,
                        to: TxKind::Call(account(number)),
                        value: U256::from(number),
                        ..Default::default()
                    }),
                    Signature::new(U256::from(1), U256::from(1), false),
                )]
            };
            let receipts = transactions
                .iter()
                .map(|_| Receipt {
                    tx_type: TxType::Legacy,
                    success: true,
                    cumulative_gas_used: 21_000,
                    logs: Vec::new(),
                })
                .collect::<Vec<_>>();
            let gas_used = transactions.len() as u64 * 21_000;
            let state = if number == 0 {
                BundleState::default()
            } else {
                BundleState::builder(number..=number)
                    .state_present_account_info(
                        account(number),
                        AccountInfo {
                            nonce: number,
                            balance: U256::from(number),
                            ..Default::default()
                        },
                    )
                    .revert_account_info(number, account(number), Some(None))
                    .state_storage(
                        account(number),
                        std::iter::once((U256::from(1), (U256::ZERO, U256::from(number))))
                            .collect(),
                    )
                    .revert_storage(number, account(number), vec![(U256::from(1), U256::ZERO)])
                    .build()
            };
            let hashed_state =
                HashedPostState::from_bundle_state::<KeccakKeyHasher>(state.state()).into_sorted();
            let block = Block {
                header: Header {
                    number,
                    parent_hash,
                    timestamp: number,
                    gas_limit: 30_000_000,
                    gas_used,
                    transactions_root: calculate_transaction_root(&transactions),
                    receipts_root: calculate_receipt_root_no_memo(&receipts),
                    ..Default::default()
                },
                body: BlockBody { transactions, ..Default::default() },
            }
            .seal_slow();
            parent_hash = block.hash();
            ExecutedBlock::new(
                Arc::new(RecoveredBlock::new_sealed(
                    block,
                    if number == 0 { Vec::new() } else { vec![account(number)] },
                )),
                Arc::new(BlockExecutionOutput {
                    result: BlockExecutionResult { receipts, gas_used, ..Default::default() },
                    state,
                }),
                ComputedTrieData {
                    sorted: SortedTrieData::new(Arc::new(hashed_state), Default::default()),
                },
            )
        })
        .collect()
}

fn assert_prefix(factory: &Factory, blocks: &[ExecutedBlock], tip: u64) {
    let reader = factory.provider().unwrap();
    assert_eq!(reader.get_stage_checkpoint(StageId::Finish).unwrap().unwrap().block_number, tip);
    for segment in [
        StaticFileSegment::Headers,
        StaticFileSegment::Transactions,
        StaticFileSegment::Receipts,
        StaticFileSegment::AccountChangeSets,
        StaticFileSegment::StorageChangeSets,
    ] {
        assert_eq!(
            factory.static_file_provider().get_highest_static_file_block(segment),
            Some(tip)
        );
    }
    for number in 0..=tip {
        let block = &blocks[number as usize];
        assert_eq!(reader.block_hash(number).unwrap(), Some(block.recovered_block().hash()));
        assert_eq!(
            reader.receipts_by_block(number.into()).unwrap().unwrap(),
            block.execution_outcome().result.receipts
        );
    }
    for number in 1..=3 {
        let block = &blocks[number as usize];
        let tx = &block.recovered_block().body().transactions[0];
        let exists = number <= tip;
        assert_eq!(reader.transaction_id(*tx.tx_hash()).unwrap(), exists.then_some(number - 1));
        assert_eq!(
            reader.basic_account(&account(number)).unwrap().map(|account| account.nonce),
            exists.then_some(number)
        );
        let account_history = factory
            .rocksdb_provider()
            .get::<tables::AccountsHistory>(ShardedKey::last(account(number)))
            .unwrap()
            .map(|history| history.iter().collect::<Vec<_>>());
        assert_eq!(account_history, exists.then_some(vec![number]));
        let storage_history = factory
            .rocksdb_provider()
            .get::<tables::StoragesHistory>(StorageShardedKey::new(
                account(number),
                B256::with_last_byte(1),
                u64::MAX,
            ))
            .unwrap()
            .map(|history| history.iter().collect::<Vec<_>>());
        assert_eq!(storage_history, exists.then_some(vec![number]));
    }
    assert!(reader.header_by_number(tip + 1).unwrap().is_none());
}

#[test]
#[ignore = "invoked in a subprocess by real_storage_commit_crash_recovery"]
fn commit_crash_child() {
    let cut = std::env::var(CHILD_CUT).expect("child needs an explicit crash cut");
    let path = std::env::var_os(CHILD_DIR).expect("child needs a parent-owned datadir");
    let factory = open_factory(Path::new(&path));
    let blocks = blocks();
    factory.set_storage_settings_cache(StorageSettings::v2());
    let writer = factory.provider_rw().unwrap();
    writer.write_storage_settings(StorageSettings::v2()).unwrap();
    save_genesis(&writer, &blocks[0]).unwrap();
    writer.commit().unwrap();
    let writer = factory.provider_rw().unwrap();
    writer.save_blocks(&SaveBlocksInput::new(vec![blocks[1].clone()], 0, 0, 1, 1)).unwrap();
    writer.commit().unwrap();
    assert_prefix(&factory, &blocks, 1);

    let writer = factory.provider_rw().unwrap();
    writer.save_blocks(&SaveBlocksInput::new(vec![blocks[2].clone()], 1, 1, 2, 2)).unwrap();
    // Each batch has data: transaction lookup, account history, and storage history. Their
    // preparation runs in parallel, so recovery must not depend on which batch arrives first.
    assert_eq!(writer.pending_rocksdb_batches.lock().len(), 3);
    assert!(writer.pending_rocksdb_batches.lock().iter().all(|batch| !batch.is_empty()));
    let target = match cut.as_str() {
        "static_files" => ("static_files", 0),
        "rocksdb_0" => ("rocksdb", 0),
        "rocksdb_1" => ("rocksdb", 1),
        "rocksdb_2" => ("rocksdb", 2),
        "mdbx" => ("mdbx", 0),
        _ => panic!("unknown crash cut {cut}"),
    };
    CUT.set(Some(target));
    writer.commit().unwrap();
    panic!("commit did not reach requested crash cut {cut}");
}

#[test]
fn real_storage_commit_crash_recovery() {
    let child_test = format!(
        "{}::commit_crash_child",
        module_path!().split_once("::").expect("test module includes crate name").1
    );
    let blocks = blocks();
    for cut in ["static_files", "rocksdb_0", "rocksdb_1", "rocksdb_2", "mdbx"] {
        eprintln!("real storage process crash cut={cut}");
        let directory = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &child_test, "--ignored", "--nocapture"])
            .env(CHILD_CUT, cut)
            .env(CHILD_DIR, directory.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(EXIT_AT_CUT),
            "cut {cut} did not exit at its hook:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // All cuts retain the acknowledged block. A commit that reached MDBX may survive even
        // though its caller never received a return; earlier cuts must discard the incomplete tail.
        let expected_tip = if cut == "mdbx" { 2 } else { 1 };
        let factory = open_factory(directory.path());
        assert!(factory.cached_storage_settings().storage_v2);
        assert_eq!(
            factory
                .provider()
                .unwrap()
                .get_stage_checkpoint(StageId::Finish)
                .unwrap()
                .unwrap()
                .block_number,
            expected_tip
        );
        assert_eq!(
            factory
                .static_file_provider()
                .get_highest_static_file_block(StaticFileSegment::Headers),
            Some(2),
            "the static-file phase must have completed before the cut"
        );
        let rocksdb = factory.rocksdb_provider();
        let persisted_indices = [
            rocksdb
                .get::<tables::TransactionHashNumbers>(
                    *blocks[2].recovered_block().body().transactions[0].tx_hash(),
                )
                .unwrap()
                .is_some(),
            rocksdb.get::<tables::AccountsHistory>(ShardedKey::last(account(2))).unwrap().is_some(),
            rocksdb
                .get::<tables::StoragesHistory>(StorageShardedKey::new(
                    account(2),
                    B256::with_last_byte(1),
                    u64::MAX,
                ))
                .unwrap()
                .is_some(),
        ]
        .into_iter()
        .filter(|&present| present)
        .count();
        let expected_indices = match cut {
            "static_files" => 0,
            "rocksdb_0" => 1,
            "rocksdb_1" => 2,
            "rocksdb_2" | "mdbx" => 3,
            _ => unreachable!(),
        };
        assert_eq!(persisted_indices, expected_indices, "cut {cut} must follow its batch write");
        drop(rocksdb);
        assert_eq!(factory.check_consistency().unwrap(), (None, None));
        assert_prefix(&factory, &blocks, expected_tip);
        assert_eq!(factory.check_consistency().unwrap(), (None, None));
        drop(factory);

        // Reopen once more to check that healing itself persisted, then advance the real writer.
        let factory = open_factory(directory.path());
        assert_prefix(&factory, &blocks, expected_tip);
        let writer = factory.provider_rw().unwrap();
        writer
            .save_blocks(&SaveBlocksInput::new(
                blocks[expected_tip as usize + 1..].to_vec(),
                expected_tip,
                expected_tip,
                3,
                3,
            ))
            .unwrap();
        writer.commit().unwrap();
        assert_prefix(&factory, &blocks, 3);
        drop(factory);
    }
}
