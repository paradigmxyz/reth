use reth_db::DatabaseEnvRO;
use reth_primitives::{
    snapshot::{Compression, Filters},
    ChainSpec, SnapshotSegment,
};
use reth_provider::{DatabaseProviderRO, ProviderFactory};
use std::{sync::Arc, time::Instant};

#[derive(Debug)]
pub(crate) enum BenchKind {
    Walk,
    RandomAll,
    RandomOne,
    RandomHash,
}

pub(crate) fn bench<F1, F2>(
    bench_kind: BenchKind,
    db: (DatabaseEnvRO, Arc<ChainSpec>),
    segment: SnapshotSegment,
    filters: Filters,
    compression: Compression,
    mut snapshot_method: F1,
    database_method: F2,
) -> eyre::Result<()>
where
    F1: FnMut() -> eyre::Result<()>,
    F2: Fn(DatabaseProviderRO<'_, DatabaseEnvRO>) -> eyre::Result<()>,
{
    let (db, chain) = db;

    println!();
    println!("############");
    println!("## [{segment:?}] [{compression:?}] [{filters:?}] [{bench_kind:?}]");
    {
        let start = Instant::now();
        snapshot_method()?;
        let end = start.elapsed().as_micros();
        println!("# snapshot {bench_kind:?} | {end} μs");
    }
    {
        let factory = ProviderFactory::new(db, chain);
        let provider = factory.provider()?;
        let start = Instant::now();
        database_method(provider)?;
        let end = start.elapsed().as_micros();
        println!("# database {bench_kind:?} | {end} μs");
    }

    Ok(())
}
