#![allow(dead_code, unused_imports, non_snake_case)]

use criterion::{
    async_executor::FuturesExecutor, black_box, criterion_group, criterion_main,
    measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{
    arbitrary::Arbitrary,
    collection::vec as proptest_vec,
    prelude::{any_with, ProptestConfig},
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_db::{
    cursor::{DbDupCursorRO, DbDupCursorRW},
    database::Database,
    mdbx::{test_utils::create_test_db_with_path, Env, EnvKind, WriteMap},
};
use reth_primitives::{Header, SealedBlock, TransactionSigned};
use reth_stages::{
    stages::TransactionLookupStage, test_utils::TestTransaction, ExecInput, Stage, StageId,
    StageSetBuilder, UnwindInput,
};
use std::{path::Path, sync::Arc, time::Instant};

criterion_group!(benches, stages);
criterion_main!(benches);

const NUM_BLOCKS: usize = 2000;

pub fn stages(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stages");
    group.measurement_time(std::time::Duration::from_millis(2000));
    group.warm_up_time(std::time::Duration::from_millis(2000));

    let tx = prepare_blocks(NUM_BLOCKS).unwrap();

    measure_txlookup_stage(&mut group, tx);
}

fn measure_txlookup_stage(group: &mut BenchmarkGroup<WallTime>, tx: TestTransaction) {
    let stage = TransactionLookupStage::new(NUM_BLOCKS as u64);
    let mut input = ExecInput::default();
    input.previous_stage = Some((StageId("Another"), NUM_BLOCKS as u64));

    group.bench_function(stringify!(TransactionLookupStage), move |b| {
        b.to_async(FuturesExecutor).iter_with_setup(
            || {
                // criterion setup does not support async, so we have to use our own runtime
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    let mut stage = stage.clone();
                    let mut db_tx = tx.inner();
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

fn prepare_blocks(num_blocks: usize) -> eyre::Result<TestTransaction> {
    let path = "testdata/stages";
    let file_path = Path::new("testdata/stages/blocks");
    let bench_db_path = "/tmp/reth-benches-stages";

    let blocks = if file_path.exists() {
        serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(file_path)?))?
    } else {
        generate_blocks(num_blocks, path, file_path)?
    };

    println!("\n## Preparing DB `{}`. \n", file_path.display());

    // Reset DB
    let _ = std::fs::remove_dir_all(bench_db_path);
    let tx = TestTransaction {
        tx: Arc::new(create_test_db_with_path::<WriteMap>(EnvKind::RW, Path::new(bench_db_path))),
    };

    tx.insert_blocks(blocks.iter(), None)?;
    tx.insert_headers(blocks.iter().map(|block| &block.header))?;

    Ok(tx)
}

fn generate_blocks(
    num_blocks: usize,
    path: &str,
    file_path: &Path,
) -> eyre::Result<Vec<SealedBlock>> {
    let mut runner = TestRunner::new(ProptestConfig::default());

    std::fs::create_dir_all(path)?;

    println!("\n## Generating blocks into `{}`. \n", file_path.display());

    // We want to generate each element of the struct separately, instead of
    // `any_with::<SealdedBlock>()`, because otherwise .ommers and .body might just generate too
    // many values unnecessarily.
    let block_header_strat = any_with::<Header>(<Header as Arbitrary>::Parameters::default());
    let ommers_strat =
        proptest_vec(any_with::<Header>(<Header as Arbitrary>::Parameters::default()), 0..10);
    let body_strat = proptest_vec(
        any_with::<TransactionSigned>(<TransactionSigned as Arbitrary>::Parameters::default()),
        0..10,
    );

    let mut blocks = vec![];
    for i in 0..num_blocks {
        // Generate random blocks
        let (mut header, ommers, body) = (&block_header_strat, &ommers_strat, &body_strat)
            .new_tree(&mut runner)
            .unwrap()
            .current();

        // Fix the Header.number since it's random
        let ommers = ommers
            .into_iter()
            .map(|mut ommer| {
                ommer.number = i as u64;
                ommer.seal()
            })
            .collect();

        header.number = i as u64;

        blocks.push(SealedBlock { header: header.seal(), ommers, body });
    }

    serde_json::to_writer_pretty(
        std::io::BufWriter::new(std::fs::File::create(file_path)?),
        &blocks,
    )
    .unwrap();

    Ok(blocks)
}
