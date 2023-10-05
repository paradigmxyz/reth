use super::{Compression, PerfectHashingFunction, Snapshots};
use crate::utils::DbTool;
use reth_db::DatabaseEnvRO;
use reth_provider::{DatabaseProviderRO, ProviderFactory};
use std::time::Instant;

#[derive(Debug)]
pub(crate) enum BenchKind {
    Walk,
    RandomAll,
    RandomOne,
}

pub(crate) fn bench<F1, F2>(
    mode: Snapshots,
    bench_kind: BenchKind,
    compression: &Compression,
    phf: &PerfectHashingFunction,
    tool: &DbTool<'_, DatabaseEnvRO>,
    mut snapshot_method: F1,
    database_method: F2,
) where
    F1: FnMut(),
    F2: Fn(DatabaseProviderRO<'_, DatabaseEnvRO>),
{
    println!();
    println!("############");
    println!("## [{mode:?}] [{compression:?}] [{phf:?}] [{bench_kind:?}]");
    {
        let start = Instant::now();
        snapshot_method();
        let end = start.elapsed().as_micros();
        println!("# snapshot {bench_kind:?} | {end} μs");
    }
    {
        let factory = ProviderFactory::new(tool.db, tool.chain.clone());
        let provider = factory.provider().unwrap();
        let start = Instant::now();
        database_method(provider);
        let end = start.elapsed().as_micros();
        println!("# database {bench_kind:?} | {end} μs");
    }
}
