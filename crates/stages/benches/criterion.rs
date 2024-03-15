#![allow(missing_docs)]
use criterion::{
    async_executor::FuturesExecutor, criterion_group, criterion_main, measurement::WallTime,
    BenchmarkGroup, Criterion,
};
use pprof::criterion::{Output, PProfProfiler};
use reth_config::config::EtlConfig;
use reth_db::{test_utils::TempDatabase, DatabaseEnv};

use reth_primitives::{stage::StageCheckpoint, BlockNumber};
use reth_stages::{
    stages::{MerkleStage, SenderRecoveryStage, TransactionLookupStage},
    test_utils::TestStageDB,
    ExecInput, Stage, StageExt, UnwindInput,
};
use std::{ops::RangeInclusive, sync::Arc};

mod setup;
use setup::StageRange;

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(1000, Output::Flamegraph(None)));
    targets = transaction_lookup, account_hashing, senders, merkle
}
criterion_main!(benches);

const DEFAULT_NUM_BLOCKS: u64 = 10_000;

fn account_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let (db, stage, range) = setup::prepare_account_hashing(num_blocks);

    measure_stage(&mut group, &db, setup::stage_unwind, stage, range, "AccountHashing".to_string());
}

fn senders(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    let db = setup::txs_testdata(DEFAULT_NUM_BLOCKS);

    for batch in [1000usize, 10_000, 100_000, 250_000] {
        let stage = SenderRecoveryStage { commit_threshold: DEFAULT_NUM_BLOCKS };
        let label = format!("SendersRecovery-batch-{batch}");

        measure_stage(&mut group, &db, setup::stage_unwind, stage, 0..=DEFAULT_NUM_BLOCKS, label);
    }
}

fn transaction_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);
    let stage = TransactionLookupStage::new(DEFAULT_NUM_BLOCKS, EtlConfig::default(), None);

    let db = setup::txs_testdata(DEFAULT_NUM_BLOCKS);

    measure_stage(
        &mut group,
        &db,
        setup::stage_unwind,
        stage,
        0..=DEFAULT_NUM_BLOCKS,
        "TransactionLookup".to_string(),
    );
}

fn merkle(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    let db = setup::txs_testdata(DEFAULT_NUM_BLOCKS);

    let stage = MerkleStage::Both { clean_threshold: u64::MAX };
    measure_stage(
        &mut group,
        &db,
        setup::unwind_hashes,
        stage,
        1..=DEFAULT_NUM_BLOCKS,
        "Merkle-incremental".to_string(),
    );

    let stage = MerkleStage::Both { clean_threshold: 0 };
    measure_stage(
        &mut group,
        &db,
        setup::unwind_hashes,
        stage,
        1..=DEFAULT_NUM_BLOCKS,
        "Merkle-fullhash".to_string(),
    );
}

fn measure_stage<F, S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    db: &TestStageDB,
    setup: F,
    stage: S,
    block_interval: RangeInclusive<BlockNumber>,
    label: String,
) where
    S: Clone + Stage<Arc<TempDatabase<DatabaseEnv>>>,
    F: Fn(S, &TestStageDB, StageRange),
{
    let stage_range = (
        ExecInput {
            target: Some(*block_interval.end()),
            checkpoint: Some(StageCheckpoint::new(*block_interval.start())),
        },
        UnwindInput {
            checkpoint: StageCheckpoint::new(*block_interval.end()),
            unwind_to: *block_interval.start(),
            bad_block: None,
        },
    );
    let (input, _) = stage_range;

    group.bench_function(label, move |b| {
        b.to_async(FuturesExecutor).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                setup(stage.clone(), db, stage_range)
            },
            |_| async {
                let mut stage = stage.clone();
                let provider = db.factory.provider_rw().unwrap();
                stage
                    .execute_ready(input)
                    .await
                    .and_then(|_| stage.execute(&provider, input))
                    .unwrap();
                provider.commit().unwrap();
            },
        )
    });
}
