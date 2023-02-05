use criterion::{
    async_executor::FuturesExecutor, criterion_group, criterion_main, measurement::WallTime,
    BenchmarkGroup, Criterion,
};
use reth_db::mdbx::{Env, WriteMap};
use reth_primitives::H256;
use reth_stages::{
    stages::{SenderRecoveryStage, TransactionLookupStage},
    test_utils::TestTransaction,
    ExecInput, Stage, StageId, UnwindInput,
};
use std::path::{Path, PathBuf};

criterion_group!(benches, tx_lookup, senders);
criterion_main!(benches);

fn senders(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    group.measurement_time(std::time::Duration::from_millis(2000));
    group.warm_up_time(std::time::Duration::from_millis(2000));
    // don't need to run each stage for that many times
    group.sample_size(10);

    for batch in [1000usize, 10_000, 100_000, 250_000] {
        let num_blocks = 10_000;
        let mut stage = SenderRecoveryStage::default();
        stage.batch_size = batch;
        stage.commit_threshold = num_blocks;
        let label = format!("SendersRecovery-batch-{}", batch);
        measure_stage(&mut group, stage, num_blocks - 1 /* why do we need - 1 here? */, label);
    }
}

fn tx_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    group.measurement_time(std::time::Duration::from_millis(2000));
    group.warm_up_time(std::time::Duration::from_millis(2000));
    // don't need to run each stage for that many times
    group.sample_size(10);

    let num_blocks = 10_000;
    let stage = TransactionLookupStage::new(num_blocks);
    measure_stage(&mut group, stage, num_blocks, "TransactionLookup".to_string());
}

fn measure_stage<S: Clone + Default + Stage<Env<WriteMap>>>(
    group: &mut BenchmarkGroup<WallTime>,
    stage: S,
    num_blocks: u64,
    label: String,
) {
    let path = txs_testdata(num_blocks as usize);
    let tx = TestTransaction::new(&path);

    let mut input = ExecInput::default();
    input.previous_stage = Some((StageId("Another"), num_blocks));

    group.bench_function(label, move |b| {
        b.to_async(FuturesExecutor).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    let mut stage = stage.clone();
                    let mut db_tx = tx.inner();

                    // Clear previous run
                    stage.unwind(&mut db_tx, UnwindInput::default()).await.unwrap();

                    db_tx.commit().unwrap();
                });
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

use reth_interfaces::test_utils::generators::random_block_range;

// Helper for generating testdata for the sender recovery stage and tx lookup stages (512MB).
// Returns the path to the database file and the number of blocks written.
fn txs_testdata(num_blocks: usize) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("txs-bench");
    let txs_range = 100..150;

    if !path.exists() {
        // create the dirs
        std::fs::create_dir_all(&path).unwrap();
        println!("Transactions testdata not found, generating to {:?}", path.display());
        let tx = TestTransaction::new(&path);

        // This takes a while because it does sig recovery internally
        let blocks = random_block_range(0..num_blocks as u64, H256::zero(), txs_range);
        tx.insert_blocks(blocks.iter(), None).unwrap();
        tx.inner().commit().unwrap();
    }

    path
}
