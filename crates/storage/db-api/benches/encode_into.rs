use alloy_primitives::{Address, B256};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use reth_db_api::table::{Encode, EncodeInto};

fn bench_encode_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode");
    group.throughput(Throughput::Elements(10000));

    let addresses: Vec<Address> = (0..10000u64)
        .map(|i| Address::from_word(B256::from(i.to_be_bytes().repeat(4).try_into().unwrap())))
        .collect();

    group.bench_function("Address::encode (returns array)", |b| {
        b.iter(|| {
            for addr in &addresses {
                let encoded = black_box(addr.clone().encode());
                black_box(encoded);
            }
        })
    });

    group.bench_function("Address::encode_into (zero-copy)", |b| {
        let mut buf = [0u8; 20];
        b.iter(|| {
            for addr in &addresses {
                addr.encode_into(&mut buf);
                black_box(&buf);
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_encode_methods);
criterion_main!(benches);
