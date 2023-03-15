use criterion::{
    async_executor::FuturesExecutor, criterion_group, criterion_main, measurement::WallTime,
    BenchmarkGroup, Criterion,
};
use pprof::criterion::{Output, PProfProfiler};
use reth_db::mdbx::{Env, WriteMap};
use reth_interfaces::test_utils::TestConsensus;
use reth_stages::{
    stages::{MerkleStage, SenderRecoveryStage, TotalDifficultyStage, TransactionLookupStage},
    test_utils::TestTransaction,
    ExecInput, Stage, StageId, UnwindInput,
};
use std::{path::PathBuf, sync::Arc};

mod setup;
use setup::StageRange;

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(1000, Output::Flamegraph(None)));
    targets = transaction_lookup, account_hashing, senders, total_difficulty, merkle
}
criterion_main!(benches);

const DEFAULT_NUM_BLOCKS: u64 = 10_000;

fn account_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let (path, stage, execution_range) = setup::prepare_account_hashing(num_blocks);

    measure_stage_with_path(
        path,
        &mut group,
        setup::stage_unwind,
        stage,
        execution_range,
        "AccountHashing".to_string(),
    );
}

fn senders(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    for batch in [1000usize, 10_000, 100_000, 250_000] {
        let stage = SenderRecoveryStage { commit_threshold: DEFAULT_NUM_BLOCKS };
        let label = format!("SendersRecovery-batch-{batch}");

        measure_stage(&mut group, setup::stage_unwind, stage, 0..DEFAULT_NUM_BLOCKS, label);
    }
}

fn transaction_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);
    let stage = TransactionLookupStage::new(DEFAULT_NUM_BLOCKS);

    measure_stage(
        &mut group,
        setup::stage_unwind,
        stage,
        0..DEFAULT_NUM_BLOCKS,
        "TransactionLookup".to_string(),
    );
}

fn total_difficulty(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    group.measurement_time(std::time::Duration::from_millis(2000));
    group.warm_up_time(std::time::Duration::from_millis(2000));
    // don't need to run each stage for that many times
    group.sample_size(10);
    let stage = TotalDifficultyStage::new(Arc::new(TestConsensus::default()));

    measure_stage(
        &mut group,
        setup::stage_unwind,
        stage,
        0..DEFAULT_NUM_BLOCKS,
        "TotalDifficulty".to_string(),
    );
}

fn merkle(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    let stage = MerkleStage::Both { clean_threshold: u64::MAX };
    measure_stage(
        &mut group,
        setup::unwind_hashes,
        stage,
        1..DEFAULT_NUM_BLOCKS + 1,
        "Merkle-incremental".to_string(),
    );

    let stage = MerkleStage::Both { clean_threshold: 0 };
    measure_stage(
        &mut group,
        setup::unwind_hashes,
        stage,
        1..DEFAULT_NUM_BLOCKS + 1,
        "Merkle-fullhash".to_string(),
    );
}

fn measure_stage_with_path<F, S>(
    path: PathBuf,
    group: &mut BenchmarkGroup<WallTime>,
    setup: F,
    stage: S,
    stage_range: StageRange,
    label: String,
) where
    S: Clone + Stage<Env<WriteMap>>,
    F: Fn(S, &TestTransaction, StageRange),
{
    let tx = TestTransaction::new(&path);
    let (input, _) = stage_range;

    group.bench_function(label, move |b| {
        b.to_async(FuturesExecutor).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                setup(stage.clone(), &tx, stage_range)
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

fn measure_stage<F, S>(
    group: &mut BenchmarkGroup<WallTime>,
    setup: F,
    stage: S,
    block_interval: std::ops::Range<u64>,
    label: String,
) where
    S: Clone + Stage<Env<WriteMap>>,
    F: Fn(S, &TestTransaction, StageRange),
{
    let path = setup::txs_testdata(block_interval.end);

    measure_stage_with_path(
        path,
        group,
        setup,
        stage,
        (
            ExecInput {
                previous_stage: Some((StageId("Another"), block_interval.end)),
                stage_progress: Some(block_interval.start),
            },
            UnwindInput {
                stage_progress: block_interval.end,
                unwind_to: block_interval.start,
                bad_block: None,
            },
        ),
        label,
    );
}
