use reth_db::DatabaseEnv;
use reth_primitives::{
    static_file::{Compression, Filters},
    StaticFileSegment,
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
    provider_factory: Arc<ProviderFactory<DatabaseEnv>>,
    segment: StaticFileSegment,
    filters: Filters,
    compression: Compression,
    mut static_file_method: F1,
    database_method: F2,
) -> eyre::Result<()>
where
    F1: FnMut() -> eyre::Result<R>,
    F2: Fn(DatabaseProviderRO<DatabaseEnv>) -> eyre::Result<R>,
    R: Debug + PartialEq,
{
    println!();
    println!("############");
    println!("## [{segment:?}] [{compression:?}] [{filters:?}] [{bench_kind:?}]");
    let static_file_result = {
        let start = Instant::now();
        let result = static_file_method()?;
        let end = start.elapsed().as_micros();
        println!("# static file {bench_kind:?} | {end} μs");
        result
    };

    let db_result = {
        let provider = provider_factory.provider()?;
        let start = Instant::now();
        let result = database_method(provider)?;
        let end = start.elapsed().as_micros();
        println!("# database {bench_kind:?} | {end} μs");
        result
    };

    assert_eq!(static_file_result, db_result);

    Ok(())
}
