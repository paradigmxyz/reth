use alloy_primitives::{Address, B256};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use reth_db_api::table::{Encode, EncodeInto};

fn bench_encode_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode");
    group.throughput(Throughput::Elements(10000));

    let addresses: Vec<Address> = (0..10000u64)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[24..32].copy_from_slice(&i.to_be_bytes());
            Address::from_word(B256::from(bytes))
        })
        .collect();

    let hashes: Vec<B256> = (0..10000u64)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[24..32].copy_from_slice(&i.to_be_bytes());
            B256::from(bytes)
        })
        .collect();

    group.bench_function("Address::encode (returns array, consumes self)", |b| {
        b.iter(|| {
            for addr in &addresses {
                let encoded = black_box((*addr).encode());
                black_box(encoded);
            }
        })
    });

    group.bench_function("Address::encode_into (zero-copy, borrows self)", |b| {
        let mut buf = [0u8; 20];
        b.iter(|| {
            for addr in &addresses {
                addr.encode_into(&mut buf);
                black_box(&buf);
            }
        })
    });

    group.bench_function("B256::encode (returns array, consumes self)", |b| {
        b.iter(|| {
            for hash in &hashes {
                let encoded = black_box((*hash).encode());
                black_box(encoded);
            }
        })
    });

    group.bench_function("B256::encode_into (zero-copy, borrows self)", |b| {
        let mut buf = [0u8; 32];
        b.iter(|| {
            for hash in &hashes {
                hash.encode_into(&mut buf);
                black_box(&buf);
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_encode_methods);
criterion_main!(benches);
