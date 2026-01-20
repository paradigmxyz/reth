//! Benchmarks for ShardedKey and StorageShardedKey encoding.
//!
//! These benchmarks measure the performance of stack-allocated vs heap-allocated key encoding,
//! inspired by Anza Labs' PR #3603 which saved ~20k allocations/sec by moving RocksDB keys
//! from heap to stack.
//!
//! Run with: `cargo bench -p reth-db-api --bench sharded_key_encode`

#![allow(missing_docs)]

use alloy_primitives::{Address, B256};
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use reth_db_api::{
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::Encode,
};

/// Number of keys to encode per iteration for throughput measurement.
const BATCH_SIZE: usize = 10_000;

fn bench_sharded_key_address_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("sharded_key_encode");
    group.throughput(Throughput::Elements(BATCH_SIZE as u64));

    // Pre-generate test data
    let keys: Vec<ShardedKey<Address>> = (0..BATCH_SIZE)
        .map(|i| {
            let mut addr_bytes = [0u8; 20];
            addr_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            ShardedKey::new(Address::from(addr_bytes), i as u64)
        })
        .collect();

    group.bench_function("ShardedKey<Address>::encode", |b| {
        b.iter_batched(
            || keys.clone(),
            |keys| {
                for key in keys {
                    let encoded = black_box(key.encode());
                    black_box(encoded.as_ref());
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_storage_sharded_key_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_sharded_key_encode");
    group.throughput(Throughput::Elements(BATCH_SIZE as u64));

    // Pre-generate test data
    let keys: Vec<StorageShardedKey> = (0..BATCH_SIZE)
        .map(|i| {
            let mut addr_bytes = [0u8; 20];
            addr_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            let mut key_bytes = [0u8; 32];
            key_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            StorageShardedKey::new(Address::from(addr_bytes), B256::from(key_bytes), i as u64)
        })
        .collect();

    group.bench_function("StorageShardedKey::encode", |b| {
        b.iter_batched(
            || keys.clone(),
            |keys| {
                for key in keys {
                    let encoded = black_box(key.encode());
                    black_box(encoded.as_ref());
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_encode_decode_roundtrip(c: &mut Criterion) {
    use reth_db_api::table::Decode;

    let mut group = c.benchmark_group("sharded_key_roundtrip");
    group.throughput(Throughput::Elements(BATCH_SIZE as u64));

    let keys: Vec<ShardedKey<Address>> = (0..BATCH_SIZE)
        .map(|i| {
            let mut addr_bytes = [0u8; 20];
            addr_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            ShardedKey::new(Address::from(addr_bytes), i as u64)
        })
        .collect();

    group.bench_function("ShardedKey<Address>::encode_then_decode", |b| {
        b.iter_batched(
            || keys.clone(),
            |keys| {
                for key in keys {
                    let encoded = key.encode();
                    let decoded = black_box(ShardedKey::<Address>::decode(&encoded).unwrap());
                    black_box(decoded);
                }
            },
            BatchSize::SmallInput,
        )
    });

    let storage_keys: Vec<StorageShardedKey> = (0..BATCH_SIZE)
        .map(|i| {
            let mut addr_bytes = [0u8; 20];
            addr_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            let mut key_bytes = [0u8; 32];
            key_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            StorageShardedKey::new(Address::from(addr_bytes), B256::from(key_bytes), i as u64)
        })
        .collect();

    group.bench_function("StorageShardedKey::encode_then_decode", |b| {
        b.iter_batched(
            || storage_keys.clone(),
            |keys| {
                for key in keys {
                    let encoded = key.encode();
                    let decoded = black_box(StorageShardedKey::decode(&encoded).unwrap());
                    black_box(decoded);
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_sharded_key_address_encode,
    bench_storage_sharded_key_encode,
    bench_encode_decode_roundtrip,
);
criterion_main!(benches);
