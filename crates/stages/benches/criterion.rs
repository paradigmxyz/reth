use criterion::{
    async_executor::FuturesExecutor, criterion_group, criterion_main, measurement::WallTime,
    BenchmarkGroup, Criterion,
};
use reth_db::mdbx::{Env, WriteMap};
use reth_stages::{
    stages::{SenderRecoveryStage, TotalDifficultyStage, TransactionLookupStage},
    test_utils::TestTransaction,
    ExecInput, Stage, StageId, UnwindInput,
};
use std::path::PathBuf;

mod setup;

// criterion_group!(benches, account_hashing, transaction_lookup, senders, total_difficulty);
criterion_group!(benches, account_hashing);
criterion_main!(benches);

fn account_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let (path, stage, execution_range) = setup::prepare_account_hashing(num_blocks);

    measure_stage_with_path(
        &mut group,
        stage,
        num_blocks,
        path,
        "AccountHashing".to_string(),
        execution_range,
    );
}

fn senders(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    for batch in [1000usize, 10_000, 100_000, 250_000] {
        let num_blocks = 10_000;
        let mut stage = SenderRecoveryStage::default();
        stage.commit_threshold = num_blocks;
        let label = format!("SendersRecovery-batch-{batch}");
        measure_stage(&mut group, stage, num_blocks, label);
    }
}

fn transaction_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let stage = TransactionLookupStage::new(num_blocks);
    measure_stage(&mut group, stage, num_blocks, "TransactionLookup".to_string());
}

fn total_difficulty(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    group.measurement_time(std::time::Duration::from_millis(2000));
    group.warm_up_time(std::time::Duration::from_millis(2000));
    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let stage = TotalDifficultyStage::default();
    measure_stage(&mut group, stage, num_blocks, "TotalDifficulty".to_string());
}

fn measure_stage_with_path<S: Clone + Default + Stage<Env<WriteMap>>>(
    group: &mut BenchmarkGroup<WallTime>,
    stage: S,
    num_blocks: u64,
    path: PathBuf,
    label: String,
    execution_range: Option<(ExecInput, UnwindInput)>,
) {
    let tx = TestTransaction::new(&path);

    let (input, unwind) = execution_range.unwrap_or_else(|| {
        let mut input = ExecInput::default();
        input.previous_stage = Some((StageId("Another"), num_blocks));
        (input, UnwindInput::default())
    });

    group.bench_function(label, move |b| {
        b.to_async(FuturesExecutor).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    let mut stage = stage.clone();
                    let mut db_tx = tx.inner();

                    // Clear previous run
                    stage.unwind(&mut db_tx, unwind).await.unwrap();

                    db_tx.commit().unwrap();
                });
            },
            |_| async {
                let mut stage = stage.clone();
                let mut db_tx = tx.inner();
                stage.execute(&mut db_tx, input).await.unwrap();
                db_tx.commit().unwrap();
            },
        )
    });
}

fn measure_stage<S: Clone + Default + Stage<Env<WriteMap>>>(
    group: &mut BenchmarkGroup<WallTime>,
    stage: S,
    num_blocks: u64,
    label: String,
) {
    let path = setup::txs_testdata(num_blocks as usize);
    measure_stage_with_path(group, stage, num_blocks, path, label, None)
}
