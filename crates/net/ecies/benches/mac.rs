#![allow(missing_docs)]

use alloy_primitives::B256;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_ecies::mac::MAC;

/// Benchmarks the `RLPx` MAC construction as used by the ECIES frame codec: one
/// `update_header` + `digest` per frame header and one `update_body` + `digest` per frame body.
fn mac_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("ecies_mac");

    group.bench_function("update_header", |b| {
        let mut mac = MAC::new(B256::repeat_byte(0xab));
        let header = [0x11u8; 16];
        b.iter(|| {
            mac.update_header(&header);
            std::hint::black_box(mac.digest())
        });
    });

    for size in [128usize, 4096, 65536] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("update_body", size), &size, |b, &size| {
            let mut mac = MAC::new(B256::repeat_byte(0xab));
            let body = vec![0x22u8; size];
            b.iter(|| {
                mac.update_body(&body);
                std::hint::black_box(mac.digest())
            });
        });
    }

    group.finish();
}

criterion_group!(benches, mac_frame);
criterion_main!(benches);
