use reth_db::DatabaseEnv;
use reth_primitives::{
    snapshot::{Compression, Filters},
    ChainSpec, SnapshotSegment,
};
use reth_provider::{DatabaseProviderRO, ProviderFactory};
use std::{fmt::Debug, sync::Arc, time::Instant};

#[derive(Debug)]
pub(crate) enum BenchKind {
    Walk,
    RandomAll,
    RandomOne,
    RandomHash,
}

pub(crate) fn bench<F1, F2, R>(
    bench_kind: BenchKind,
    db: (DatabaseEnv, Arc<ChainSpec>),
    segment: SnapshotSegment,
    filters: Filters,
    compression: Compression,
    mut snapshot_method: F1,
    database_method: F2,
) -> eyre::Result<()>
where
    F1: FnMut() -> eyre::Result<R>,
    F2: Fn(DatabaseProviderRO<DatabaseEnv>) -> eyre::Result<R>,
    R: Debug + PartialEq,
{
    let (db, chain) = db;

    println!();
    println!("############");
    println!("## [{segment:?}] [{compression:?}] [{filters:?}] [{bench_kind:?}]");
    let snap_result = {
        let start = Instant::now();
        let result = snapshot_method()?;
        let end = start.elapsed().as_micros();
        println!("# snapshot {bench_kind:?} | {end} μs");
        result
    };

    let db_result = {
        let factory = ProviderFactory::new(db, chain);
        let provider = factory.provider()?;
        let start = Instant::now();
        let result = database_method(provider)?;
        let end = start.elapsed().as_micros();
        println!("# database {bench_kind:?} | {end} μs");
        result
    };

    assert_eq!(snap_result, db_result);

    Ok(())
}
