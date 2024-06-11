#![allow(missing_docs)]
use alloy_rlp::Decodable;
use criterion::{criterion_group, criterion_main, Criterion};
use pprof::criterion::{Output, PProfProfiler};
use reth_primitives::{hex_literal::hex, TransactionSigned};

/// Benchmarks the recovery of the public key from the ECDSA message using criterion.
pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("recover ECDSA", |b| {
        b.iter(|| {
            let raw =hex!("f88b8212b085028fa6ae00830f424094aad593da0c8116ef7d2d594dd6a63241bccfc26c80a48318b64b000000000000000000000000641c5d790f862a58ec7abcfd644c0442e9c201b32aa0a6ef9e170bca5ffb7ac05433b13b7043de667fbb0b4a5e45d3b54fb2d6efcc63a0037ec2c05c3d60c5f5f78244ce0a3859e3a18a36c61efb061b383507d3ce19d2");
            let mut pointer = raw.as_ref();
            let tx = TransactionSigned::decode(&mut pointer).unwrap();
            tx.recover_signer();
            }
        )
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = criterion_benchmark
}
criterion_main!(benches);
