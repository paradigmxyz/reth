#![allow(missing_docs)]
use criterion::{criterion_main, measurement::WallTime, BenchmarkGroup, Criterion};
#[cfg(not(target_os = "windows"))]
use pprof::criterion::{Output, PProfProfiler};
use reth_chainspec::ChainSpec;
use reth_config::config::{EtlConfig, TransactionLookupConfig};
use reth_db::{test_utils::TempDatabase, Database, DatabaseEnv};

use alloy_primitives::BlockNumber;
use reth_provider::{DatabaseProvider, DatabaseProviderFactory};
use reth_stages::{
    stages::{MerkleStage, SenderRecoveryStage, TransactionLookupStage},
    test_utils::TestStageDB,
    StageCheckpoint,
};
use reth_stages_api::{ExecInput, Stage, StageExt, UnwindInput};
use std::ops::RangeInclusive;
use tokio::runtime::Runtime;

mod setup;
use setup::StageRange;

// Expanded form of `criterion_group!`
//
// This is currently needed to only instantiate the tokio runtime once.
fn benches() {
    #[cfg(not(target_os = "windows"))]
    let mut criterion = Criterion::default()
        .with_profiler(PProfProfiler::new(1000, Output::Flamegraph(None)))
        .configure_from_args();

    let runtime = Runtime::new().unwrap();
    let _guard = runtime.enter();

    #[cfg(target_os = "windows")]
    let mut criterion = Criterion::default().configure_from_args();

    transaction_lookup(&mut criterion, &runtime);
    account_hashing(&mut criterion, &runtime);
    senders(&mut criterion, &runtime);
    merkle(&mut criterion, &runtime);
}

criterion_main!(benches);

const DEFAULT_NUM_BLOCKS: u64 = 10_000;

fn account_hashing(c: &mut Criterion, runtime: &Runtime) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let (db, stage, range) = setup::prepare_account_hashing(num_blocks);

    measure_stage(
        runtime,
        &mut group,
        &db,
        setup::stage_unwind,
        stage,
        range,
        "AccountHashing".to_string(),
    );
}

fn senders(c: &mut Criterion, runtime: &Runtime) {
    let mut group = c.benchmark_group("Stages");

    // don't need to run each stage for that many times
    group.sample_size(10);

    let db = setup::txs_testdata(DEFAULT_NUM_BLOCKS);

    let stage = SenderRecoveryStage { commit_threshold: DEFAULT_NUM_BLOCKS };

    measure_stage(
        runtime,
        &mut group,
        &db,
        setup::stage_unwind,
        stage,
        0..=DEFAULT_NUM_BLOCKS,
        "SendersRecovery".to_string(),
    );
}

fn transaction_lookup(c: &mut Criterion, runtime: &Runtime) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);
    let stage = TransactionLookupStage::new(
        TransactionLookupConfig { chunk_size: DEFAULT_NUM_BLOCKS },
        EtlConfig::default(),
        None,
    );

    let db = setup::txs_testdata(DEFAULT_NUM_BLOCKS);

    measure_stage(
        runtime,
        &mut group,
        &db,
        setup::stage_unwind,
        stage,
        0..=DEFAULT_NUM_BLOCKS,
        "TransactionLookup".to_string(),
    );
}

fn merkle(c: &mut Criterion, runtime: &Runtime) {
    let mut group = c.benchmark_group("Stages");
    // don't need to run each stage for that many times
    group.sample_size(10);

    let db = setup::txs_testdata(DEFAULT_NUM_BLOCKS);

    let stage = MerkleStage::Both { clean_threshold: u64::MAX };
    measure_stage(
        runtime,
        &mut group,
        &db,
        setup::unwind_hashes,
        stage,
        1..=DEFAULT_NUM_BLOCKS,
        "Merkle-incremental".to_string(),
    );

    let stage = MerkleStage::Both { clean_threshold: 0 };
    measure_stage(
        runtime,
        &mut group,
        &db,
        setup::unwind_hashes,
        stage,
        1..=DEFAULT_NUM_BLOCKS,
        "Merkle-fullhash".to_string(),
    );
}

fn measure_stage<F, S>(
    runtime: &Runtime,
    group: &mut BenchmarkGroup<'_, WallTime>,
    db: &TestStageDB,
    setup: F,
    stage: S,
    block_interval: RangeInclusive<BlockNumber>,
    label: String,
) where
    S: Clone + Stage<DatabaseProvider<<TempDatabase<DatabaseEnv> as Database>::TXMut, ChainSpec>>,
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
        b.to_async(runtime).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                setup(stage.clone(), db, stage_range)
            },
            |_| async {
                let mut stage = stage.clone();
                let provider = db.factory.database_provider_rw().unwrap();
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
