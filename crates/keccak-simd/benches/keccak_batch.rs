use alloy_primitives::{keccak256, B256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_keccak_simd::{keccak256_batch_20, keccak256_batch_32};

fn bench_batch_32(c: &mut Criterion) {
    let mut group = c.benchmark_group("keccak256_32byte");

    for count in [4, 16, 64, 256, 1024] {
        let inputs: Vec<[u8; 32]> = (0..count)
            .map(|i| {
                let mut buf = [0u8; 32];
                buf[..8].copy_from_slice(&(i as u64).to_le_bytes());
                buf
            })
            .collect();

        group.bench_with_input(BenchmarkId::new("scalar", count), &inputs, |b, inputs| {
            b.iter(|| {
                for input in inputs {
                    black_box(keccak256(input));
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("batch_4x", count), &inputs, |b, inputs| {
            let mut outputs = vec![B256::ZERO; inputs.len()];
            b.iter(|| {
                keccak256_batch_32(black_box(inputs), black_box(&mut outputs));
            })
        });
    }

    group.finish();
}

fn bench_batch_20(c: &mut Criterion) {
    let mut group = c.benchmark_group("keccak256_20byte");

    for count in [4, 16, 64, 256, 1024] {
        let inputs: Vec<[u8; 20]> = (0..count)
            .map(|i| {
                let mut buf = [0u8; 20];
                buf[..8].copy_from_slice(&(i as u64).to_le_bytes());
                buf
            })
            .collect();

        group.bench_with_input(BenchmarkId::new("scalar", count), &inputs, |b, inputs| {
            b.iter(|| {
                for input in inputs {
                    black_box(keccak256(input));
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("batch_4x", count), &inputs, |b, inputs| {
            let mut outputs = vec![B256::ZERO; inputs.len()];
            b.iter(|| {
                keccak256_batch_20(black_box(inputs), black_box(&mut outputs));
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_batch_32, bench_batch_20);
criterion_main!(benches);
