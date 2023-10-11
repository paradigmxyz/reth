use reth_db::DatabaseEnvRO;
use reth_primitives::ChainSpec;
use reth_provider::{DatabaseProviderRO, ProviderFactory};
use reth_snapshot::segments::Segment;
use std::{sync::Arc, time::Instant};

#[derive(Debug)]
pub(crate) enum BenchKind {
    Walk,
    RandomAll,
    RandomOne,
    RandomHash,
}

pub(crate) fn bench<F1, F2, S>(
    bench_kind: BenchKind,
    db: (DatabaseEnvRO, Arc<ChainSpec>),
    segment: &S,
    mut snapshot_method: F1,
    database_method: F2,
) -> eyre::Result<()>
where
    F1: FnMut() -> eyre::Result<()>,
    F2: Fn(DatabaseProviderRO<'_, DatabaseEnvRO>) -> eyre::Result<()>,
    S: Segment,
{
    let (snapshot_segment, compression, filters) =
        (segment.segment(), segment.compression(), segment.filters());
    let (db, chain) = db;

    println!();
    println!("############");
    println!("## [{snapshot_segment:?}] [{compression:?}] [{filters:?}] [{bench_kind:?}]");
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
