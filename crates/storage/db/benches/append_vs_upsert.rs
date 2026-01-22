#![allow(missing_docs)]

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_db::{test_utils::create_test_rw_db_with_path, Database};
use reth_db_api::{
    cursor::{DbCursorRW, DbDupCursorRW},
    tables::{CanonicalHeaders, PlainStorageState},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::StorageEntry;
use std::path::Path;

mod utils;
use utils::BENCH_DB_PATH;

fn bench_dupsort_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("dupsort_write_strategy");
    let bench_db_path = Path::new(BENCH_DB_PATH);

    for count in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(count as u64));

        let addresses: Vec<Address> =
            (0..10u64).map(|i| Address::with_last_byte(i as u8)).collect();

        let entries_per_address = count / 10;
        let storage_entries: Vec<Vec<StorageEntry>> = addresses
            .iter()
            .map(|_| {
                (0..entries_per_address)
                    .map(|i| StorageEntry {
                        key: B256::from(U256::from(i)),
                        value: U256::from(i * 100),
                    })
                    .collect()
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("append_dup", count),
            &(&addresses, &storage_entries),
            |b, (addrs, entries)| {
                b.iter_with_setup(
                    || {
                        let _ = std::fs::remove_dir_all(bench_db_path);
                        create_test_rw_db_with_path(bench_db_path)
                    },
                    |db| {
                        let tx = db.tx_mut().unwrap();
                        let mut cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();

                        for (addr, addr_entries) in addrs.iter().zip(entries.iter()) {
                            for entry in addr_entries {
                                cursor.append_dup(*addr, *entry).unwrap();
                            }
                        }

                        tx.commit().unwrap();
                    },
                )
            },
        );

        group.bench_with_input(
            BenchmarkId::new("upsert", count),
            &(&addresses, &storage_entries),
            |b, (addrs, entries)| {
                b.iter_with_setup(
                    || {
                        let _ = std::fs::remove_dir_all(bench_db_path);
                        create_test_rw_db_with_path(bench_db_path)
                    },
                    |db| {
                        let tx = db.tx_mut().unwrap();
                        let mut cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();

                        for (addr, addr_entries) in addrs.iter().zip(entries.iter()) {
                            for entry in addr_entries {
                                cursor.upsert(*addr, entry).unwrap();
                            }
                        }

                        tx.commit().unwrap();
                    },
                )
            },
        );

        group.bench_with_input(
            BenchmarkId::new("insert", count),
            &(&addresses, &storage_entries),
            |b, (addrs, entries)| {
                b.iter_with_setup(
                    || {
                        let _ = std::fs::remove_dir_all(bench_db_path);
                        create_test_rw_db_with_path(bench_db_path)
                    },
                    |db| {
                        let tx = db.tx_mut().unwrap();
                        let mut cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();

                        for (addr, addr_entries) in addrs.iter().zip(entries.iter()) {
                            for entry in addr_entries {
                                let _ = cursor.insert(*addr, entry);
                            }
                        }

                        tx.commit().unwrap();
                    },
                )
            },
        );
    }

    group.finish();
}

fn bench_regular_table_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("regular_table_append_vs_put");
    let bench_db_path = Path::new(BENCH_DB_PATH);

    for count in [1000u64, 10000] {
        group.throughput(Throughput::Elements(count));

        let headers: Vec<(u64, B256)> =
            (0..count).map(|i| (i, B256::from(U256::from(i)))).collect();

        group.bench_with_input(BenchmarkId::new("append", count), &headers, |b, hdrs| {
            b.iter_with_setup(
                || {
                    let _ = std::fs::remove_dir_all(bench_db_path);
                    create_test_rw_db_with_path(bench_db_path)
                },
                |db| {
                    let tx = db.tx_mut().unwrap();
                    let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

                    for (num, hash) in hdrs {
                        cursor.append(*num, hash).unwrap();
                    }

                    tx.commit().unwrap();
                },
            )
        });

        group.bench_with_input(BenchmarkId::new("upsert", count), &headers, |b, hdrs| {
            b.iter_with_setup(
                || {
                    let _ = std::fs::remove_dir_all(bench_db_path);
                    create_test_rw_db_with_path(bench_db_path)
                },
                |db| {
                    let tx = db.tx_mut().unwrap();
                    let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

                    for (num, hash) in hdrs {
                        cursor.upsert(*num, hash).unwrap();
                    }

                    tx.commit().unwrap();
                },
            )
        });
    }

    group.finish();
}

criterion_group!(benches, bench_dupsort_writes, bench_regular_table_append);
criterion_main!(benches);
