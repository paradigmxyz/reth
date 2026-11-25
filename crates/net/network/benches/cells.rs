//! Cell response validation with peer-controlled repeated hashes.

#![expect(missing_docs, reason = "Criterion generates the public benchmark entry point.")]

use alloy_primitives::{B128, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_eth_wire::{Cells, GetCells};
use std::hint::black_box;

fn validate_shared_response(c: &mut Criterion) {
    let mut group = c.benchmark_group("shared_cell_response");
    for count in [32, 64, 65, 512, 4096, 16384] {
        let request = GetCells {
            hashes: (0..count).map(|index| B256::from(U256::from(index))).collect(),
            cell_mask: B128::ZERO.into(),
        };
        let response = Cells {
            hashes: vec![request.hashes[count - 1]; count],
            cells: vec![vec![]; count],
            cell_mask: B128::ZERO.into(),
        };
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| black_box(&response).validate_response(black_box(&request)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, validate_shared_response);
criterion_main!(benches);
