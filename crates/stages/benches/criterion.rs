use criterion::{
    async_executor::FuturesExecutor, criterion_group, criterion_main, measurement::WallTime,
    BenchmarkGroup, Criterion,
};
use itertools::concat;
use reth_db::{
    cursor::DbCursorRO,
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::test_utils::generators::{
    random_block_range, random_contract_account_range, random_eoa_account_range,
    random_transition_range,
};
use reth_primitives::{Account, Address, H256};
use reth_stages::{
    stages::{
        AccountHashingStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TotalDifficultyStage, TransactionLookupStage,
    },
    test_utils::TestTransaction,
    ExecInput, Stage, StageId, UnwindInput,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

criterion_group!(benches, tx_lookup, senders, total_difficulty, merkle);
criterion_main!(benches);

fn senders(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    for batch in [1000usize, 10_000, 100_000, 250_000] {
        let num_blocks = 10_000;
        let mut stage = SenderRecoveryStage::default();
        stage.commit_threshold = num_blocks;
        let label = format!("SendersRecovery-batch-{batch}");

        let path = txs_testdata(num_blocks as usize);

        measure_stage(&mut group, stage_unwind, stage, path, label);
    }
}

fn tx_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let stage = TransactionLookupStage::new(num_blocks);

    let path = txs_testdata(num_blocks as usize);

    measure_stage(&mut group, stage_unwind, stage, path, "TransactionLookup".to_string());
}

fn total_difficulty(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    group.measurement_time(std::time::Duration::from_millis(2000));
    group.warm_up_time(std::time::Duration::from_millis(2000));
    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let stage = TotalDifficultyStage::default();

    let path = txs_testdata(num_blocks);

    measure_stage(&mut group, stage_unwind, stage, path, "TotalDifficulty".to_string());
}

fn merkle(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;

    let path = txs_testdata(num_blocks);

    let stage = MerkleStage::Both { clean_threshold: 10_001 };
    measure_stage(&mut group, unwind_hashes, stage, path.clone(), "Merkle-incremental".to_string());

    let stage = MerkleStage::Both { clean_threshold: 0 };
    measure_stage(&mut group, unwind_hashes, stage, path, "Merkle-fullhash".to_string());
}

fn stage_unwind<S: Clone + Stage<Env<WriteMap>>>(
    stage: S,
    tx: &TestTransaction,
    _exec_input: ExecInput,
) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut stage = stage.clone();
        let mut db_tx = tx.inner();

        // Clear previous run
        stage.unwind(&mut db_tx, UnwindInput::default()).await.unwrap();

        db_tx.commit().unwrap();
    });
}

fn unwind_hashes<S: Clone + Stage<Env<WriteMap>>>(
    stage: S,
    tx: &TestTransaction,
    exec_input: ExecInput,
) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut stage = stage.clone();
        let mut db_tx = tx.inner();

        StorageHashingStage::default().unwind(&mut db_tx, UnwindInput::default()).await.unwrap();
        AccountHashingStage::default().unwind(&mut db_tx, UnwindInput::default()).await.unwrap();

        // Clear previous run
        stage.unwind(&mut db_tx, UnwindInput::default()).await.unwrap();

        AccountHashingStage::default().execute(&mut db_tx, exec_input).await.unwrap();
        StorageHashingStage::default().execute(&mut db_tx, exec_input).await.unwrap();

        db_tx.commit().unwrap();
    });
}

fn measure_stage<S, F>(
    group: &mut BenchmarkGroup<WallTime>,
    setup: F,
    stage: S,
    path: PathBuf,
    label: String,
) where
    S: Clone + Stage<Env<WriteMap>>,
    F: Fn(S, &TestTransaction, ExecInput),
{
    let tx = TestTransaction::new(&path);

    let mut input = ExecInput::default();
    let (num_blocks, _) = tx
        .inner()
        .cursor_read::<tables::Headers>()
        .unwrap()
        .last()
        .unwrap()
        .expect("Headers table should not be empty");
    input.previous_stage = Some((StageId("Another"), num_blocks));

    group.bench_function(label, move |b| {
        b.to_async(FuturesExecutor).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                setup(stage.clone(), &tx, input.clone())
            },
            |_| async {
                let mut stage = stage.clone();
                let mut db_tx = tx.inner();
                stage.execute(&mut db_tx, input.clone()).await.unwrap();
                db_tx.commit().unwrap();
            },
        )
    });
}

// Helper for generating testdata for the benchmarks.
// Returns the path to the database file.
fn txs_testdata(num_blocks: usize) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("accs-bench");
    let txs_range = 100..150;

    // number of storage changes per transition
    let n_changes = 0..3;

    // range of possible values for a storage key
    let key_range = 0..300;

    // number of accounts
    let n_eoa = 131;
    let n_contract = 31;

    if !path.exists() {
        // create the dirs
        std::fs::create_dir_all(&path).unwrap();
        println!("Transactions testdata not found, generating to {:?}", path.display());
        let tx = TestTransaction::new(&path);

        let accounts: BTreeMap<Address, Account> = concat([
            random_eoa_account_range(&mut (0..n_eoa)),
            random_contract_account_range(&mut (0..n_contract)),
        ])
        .into_iter()
        .collect();

        let blocks = random_block_range(0..num_blocks as u64 + 1, H256::zero(), txs_range);

        tx.insert_blocks(blocks.iter(), None).unwrap();

        let (transitions, final_state) =
            random_transition_range(blocks.iter(), accounts, n_changes.clone(), key_range.clone());

        tx.insert_transitions(transitions).unwrap();

        tx.insert_accounts_and_storages(final_state).unwrap();

        // initialize TD
        tx.commit(|tx| {
            let (head, _) =
                tx.cursor_read::<tables::Headers>()?.first()?.unwrap_or_default().into();
            tx.put::<tables::HeaderTD>(head, reth_primitives::U256::from(0).into())
        })
        .unwrap();
    }

    path
}
